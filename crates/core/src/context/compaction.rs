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
use crate::llm::LlmMessageRole;
use crate::protocol::{AgentToolDefinition, AgentToolSafety};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
    minimum_summary_tokens: u64,
    maximum_summary_tokens: u64,
    recent_durable_units: usize,
    recent_durable_tokens: u64,
    recent_run_units: usize,
    recent_run_tokens: u64,
}

impl Default for ContextCompactionPolicy {
    fn default() -> Self {
        Self {
            soft_trigger_percent: 75,
            base_headroom_percent: 25,
            run_growth_reserve_percent: 50,
            maximum_headroom_percent: 40,
            durable_trigger_percent: 75,
            durable_target_percent: 15,
            minimum_reclaim_tokens: 512,
            maximum_reclaim_floor_tokens: 4_096,
            minimum_summary_tokens: 256,
            maximum_summary_tokens: 12_000,
            recent_durable_units: 4,
            recent_durable_tokens: 4_096,
            recent_run_units: 2,
            recent_run_tokens: 2_048,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextCompactionScope {
    RunOverlay,
    DurableHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextCompactionProtectionReason {
    FixedRequest,
    RequestOnly,
    CurrentUser,
    UserAttachment,
    RuntimeGuard,
    VisualInput,
    MixedAtomicGroup,
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
    pub(crate) scope: ContextCompactionScope,
    pub(crate) ranges: Vec<ContextCompactionItemRange>,
    pub(crate) atomic_unit_count: usize,
    pub(crate) source_input_tokens: u64,
    pub(crate) maximum_summary_tokens: u64,
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
    pub(crate) covered_message_ids: Vec<String>,
    pub(crate) covered_through_message_id: String,
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
    pub(crate) planned_durable_reclaimed_tokens: u64,
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

    #[cfg(test)]
    fn with_policy(policy: ContextCompactionPolicy, tools: &[AgentToolDefinition]) -> Self {
        Self {
            policy,
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
    ) -> ContextCompactionPlan {
        let units = build_atomic_units(items, &self.tool_safety);
        let last_durable_user_index = items
            .iter()
            .rev()
            .find(|item| {
                item.usage_class == ContextUsageClass::Durable && item.role == LlmMessageRole::User
            })
            .map(|item| item.index);
        let recent_durable = recent_unit_starts(
            &units,
            ContextUsageClass::Durable,
            self.policy.recent_durable_units,
            self.policy.recent_durable_tokens,
        );
        let recent_run = recent_unit_starts(
            &units,
            ContextUsageClass::RunTransient,
            self.policy.recent_run_units,
            self.policy.recent_run_tokens,
        );

        let mut protected_reasons = BTreeMap::<String, u64>::new();
        let mut protected_unit_count = 0_usize;
        let mut candidates = Vec::new();
        for unit in units.iter().cloned() {
            if let Some(reason) = absolute_protection_reason(&unit, last_durable_user_index) {
                protected_unit_count = protected_unit_count.saturating_add(1);
                merge_reason_tokens(&mut protected_reasons, reason, unit.tokens);
                continue;
            }
            let scope = match unit.usage_class {
                ContextUsageClass::RunTransient => ContextCompactionScope::RunOverlay,
                ContextUsageClass::Durable => ContextCompactionScope::DurableHistory,
                ContextUsageClass::Fixed | ContextUsageClass::RequestOnly => {
                    unreachable!("fixed and request-only units are protected above")
                }
            };
            let is_recent = match scope {
                ContextCompactionScope::RunOverlay => recent_run.contains(&unit.start_index),
                ContextCompactionScope::DurableHistory => {
                    recent_durable.contains(&unit.start_index)
                }
            };
            candidates.push(CompactionCandidate {
                unit,
                scope,
                priority: candidate_priority(scope, is_recent),
            });
        }
        candidates.sort_by_key(|candidate| {
            (
                candidate.priority
                    + u8::from(candidate.unit.contains_side_effects) * 2
                    + u8::from(candidate.unit.contains_errors),
                candidate.unit.start_index,
            )
        });

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
        if !request_pressure && !durable_pressure {
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
        let required_durable_reclaimed_tokens = if durable_pressure {
            durable_input_tokens.saturating_sub(durable_target_input_tokens)
        } else {
            0
        };
        let required_reclaimed_tokens =
            required_request_reclaimed_tokens.max(required_durable_reclaimed_tokens);

        let mut selected = Vec::<CompactionCandidate>::new();
        let mut selected_starts = BTreeSet::new();
        if required_durable_reclaimed_tokens > 0 {
            for candidate in candidates
                .iter()
                .filter(|candidate| candidate.scope == ContextCompactionScope::DurableHistory)
            {
                selected_starts.insert(candidate.unit.start_index);
                selected.push(candidate.clone());
                if maximum_reclaimable_tokens_for_scope(
                    &selected,
                    ContextCompactionScope::DurableHistory,
                    &self.policy,
                ) >= required_durable_reclaimed_tokens
                {
                    break;
                }
            }
        }
        let achievable_durable_reclaimed_tokens = maximum_reclaimable_tokens_for_scope(
            &selected,
            ContextCompactionScope::DurableHistory,
            &self.policy,
        );
        let executable_durable_reclaim_goal =
            required_durable_reclaimed_tokens.min(achievable_durable_reclaimed_tokens);
        let executable_reclaim_goal =
            required_request_reclaimed_tokens.max(executable_durable_reclaim_goal);
        for candidate in &candidates {
            if maximum_reclaimable_tokens(&selected, &self.policy) >= executable_reclaim_goal {
                break;
            }
            if !selected_starts.insert(candidate.unit.start_index) {
                continue;
            }
            selected.push(candidate.clone());
        }
        normalize_stable_durable_prefix(&mut selected, &units, &candidates);

        let steps = build_steps(
            &selected,
            executable_reclaim_goal,
            executable_durable_reclaim_goal,
            &self.policy,
        );
        let planned_reclaimed_tokens = steps
            .iter()
            .map(|step| step.expected_reclaimed_tokens)
            .sum::<u64>();
        let planned_durable_reclaimed_tokens = steps
            .iter()
            .filter(|step| step.scope == ContextCompactionScope::DurableHistory)
            .map(|step| step.expected_reclaimed_tokens)
            .sum::<u64>();
        let projected_request_input_tokens = query
            .request_input_tokens
            .saturating_sub(planned_reclaimed_tokens);
        let projected_durable_input_tokens =
            durable_input_tokens.saturating_sub(planned_durable_reclaimed_tokens);
        let request_target_satisfied =
            !request_pressure || projected_request_input_tokens <= target_input_tokens;
        let durable_target_satisfied =
            !durable_pressure || projected_durable_input_tokens <= durable_target_input_tokens;
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
            planned_durable_reclaimed_tokens,
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
    scope: ContextCompactionScope,
    priority: u8,
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
) -> Option<ContextCompactionProtectionReason> {
    if unit.mixed_usage_classes {
        return Some(ContextCompactionProtectionReason::MixedAtomicGroup);
    }
    match unit.usage_class {
        ContextUsageClass::Fixed => return Some(ContextCompactionProtectionReason::FixedRequest),
        ContextUsageClass::RequestOnly => {
            return Some(ContextCompactionProtectionReason::RequestOnly)
        }
        ContextUsageClass::Durable | ContextUsageClass::RunTransient => {}
    }
    if last_durable_user_index.is_some_and(|index| {
        unit.usage_class == ContextUsageClass::Durable
            && unit.start_index <= index
            && index < unit.end_index_exclusive
    }) {
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

fn recent_unit_starts(
    units: &[AtomicContextUnit],
    usage_class: ContextUsageClass,
    minimum_units: usize,
    minimum_tokens: u64,
) -> BTreeSet<usize> {
    let mut selected = BTreeSet::new();
    let mut tokens = 0_u64;
    for unit in units
        .iter()
        .rev()
        .filter(|unit| unit.usage_class == usage_class)
    {
        if selected.len() >= minimum_units && tokens >= minimum_tokens {
            break;
        }
        selected.insert(unit.start_index);
        tokens = tokens.saturating_add(unit.tokens);
    }
    selected
}

fn candidate_priority(scope: ContextCompactionScope, is_recent: bool) -> u8 {
    let scope_priority = match scope {
        ContextCompactionScope::RunOverlay => 0,
        ContextCompactionScope::DurableHistory => 1,
    };
    scope_priority + if is_recent { 8 } else { 0 }
}

fn maximum_reclaimable_tokens(
    selected: &[CompactionCandidate],
    policy: &ContextCompactionPolicy,
) -> u64 {
    [
        ContextCompactionScope::RunOverlay,
        ContextCompactionScope::DurableHistory,
    ]
    .into_iter()
    .map(|scope| maximum_reclaimable_tokens_for_scope(selected, scope, policy))
    .sum()
}

fn maximum_reclaimable_tokens_for_scope(
    selected: &[CompactionCandidate],
    scope: ContextCompactionScope,
    policy: &ContextCompactionPolicy,
) -> u64 {
    let source_tokens = selected
        .iter()
        .filter(|candidate| candidate.scope == scope)
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>();
    source_tokens.saturating_sub(minimum_summary_budget(source_tokens, policy))
}

fn build_steps(
    selected: &[CompactionCandidate],
    required_reclaimed_tokens: u64,
    required_durable_reclaimed_tokens: u64,
    policy: &ContextCompactionPolicy,
) -> Vec<ContextCompactionStep> {
    let mut by_scope = BTreeMap::<ContextCompactionScope, Vec<&CompactionCandidate>>::new();
    for candidate in selected {
        by_scope.entry(candidate.scope).or_default().push(candidate);
    }

    let source_tokens = by_scope
        .iter()
        .map(|(scope, candidates)| {
            (
                *scope,
                candidates
                    .iter()
                    .map(|candidate| candidate.unit.tokens)
                    .sum::<u64>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let maximum_summary_by_scope = source_tokens
        .iter()
        .map(|(scope, tokens)| {
            let required_scope_reclaim = match scope {
                ContextCompactionScope::RunOverlay => 0,
                ContextCompactionScope::DurableHistory => required_durable_reclaimed_tokens,
            };
            (
                *scope,
                maximum_summary_budget(*tokens, policy)
                    .min(tokens.saturating_sub(required_scope_reclaim)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let total_source_tokens = source_tokens.values().copied().sum::<u64>();
    let maximum_total_summary_tokens = total_source_tokens
        .saturating_sub(required_reclaimed_tokens)
        .min(maximum_summary_by_scope.values().copied().sum());
    let mut summary_budgets = source_tokens
        .iter()
        .map(|(scope, tokens)| (*scope, minimum_summary_budget(*tokens, policy)))
        .collect::<BTreeMap<_, _>>();
    let minimum_total_summary_tokens = summary_budgets.values().copied().sum::<u64>();
    let mut unallocated_summary_tokens =
        maximum_total_summary_tokens.saturating_sub(minimum_total_summary_tokens);

    // Preserve durable history with the spare summary budget first. Run-overlay material is
    // usually available in a richer durable projection after the turn finishes.
    for scope in [
        ContextCompactionScope::DurableHistory,
        ContextCompactionScope::RunOverlay,
    ] {
        let Some(source_tokens) = source_tokens.get(&scope).copied() else {
            continue;
        };
        let budget = summary_budgets.entry(scope).or_default();
        let additional_capacity = maximum_summary_by_scope
            .get(&scope)
            .copied()
            .unwrap_or_else(|| maximum_summary_budget(source_tokens, policy))
            .saturating_sub(*budget);
        let additional = additional_capacity.min(unallocated_summary_tokens);
        *budget = budget.saturating_add(additional);
        unallocated_summary_tokens = unallocated_summary_tokens.saturating_sub(additional);
    }

    by_scope
        .into_iter()
        .filter_map(|(scope, mut candidates)| {
            candidates.sort_by_key(|candidate| candidate.unit.start_index);
            let source_input_tokens = candidates
                .iter()
                .map(|candidate| candidate.unit.tokens)
                .sum::<u64>();
            let maximum_summary_tokens = summary_budgets.get(&scope).copied().unwrap_or_default();
            let expected_reclaimed_tokens =
                source_input_tokens.saturating_sub(maximum_summary_tokens);
            if expected_reclaimed_tokens == 0 {
                return None;
            }
            let ranges = merge_candidate_ranges(&candidates);
            Some(ContextCompactionStep {
                scope,
                ranges,
                atomic_unit_count: candidates.len(),
                source_input_tokens,
                maximum_summary_tokens,
                expected_reclaimed_tokens,
                contains_side_effects: candidates
                    .iter()
                    .any(|candidate| candidate.unit.contains_side_effects),
                contains_errors: candidates
                    .iter()
                    .any(|candidate| candidate.unit.contains_errors),
                durable_prefix: (scope == ContextCompactionScope::DurableHistory)
                    .then(|| durable_prefix_for_candidates(&candidates))
                    .flatten(),
            })
        })
        .collect()
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
        .filter(|candidate| candidate.scope == ContextCompactionScope::DurableHistory)
        .map(|candidate| candidate.unit.start_index)
        .max()
    else {
        return;
    };
    let candidates_by_start = candidates
        .iter()
        .filter(|candidate| candidate.scope == ContextCompactionScope::DurableHistory)
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
    selected.retain(|candidate| candidate.scope != ContextCompactionScope::DurableHistory);
    selected.extend(prefix);
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
    let mut covered_message_ids = Vec::new();
    let mut expect_user = true;
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
            ContextOriginKind::ConversationMessage => {
                let expected_role = if expect_user {
                    LlmMessageRole::User
                } else {
                    LlmMessageRole::Assistant
                };
                if unit.first_role != expected_role || unit.last_role != expected_role {
                    return None;
                }
                if covered_message_ids.last().map(String::as_str) != Some(origin.id()) {
                    covered_message_ids.push(origin.id().to_string());
                }
                expect_user = !expect_user;
            }
        }
    }
    if !expect_user {
        return None;
    }
    let covered_through_message_id = covered_message_ids.last()?.clone();
    Some(ContextCompactionDurablePrefix {
        previous_summary_id,
        covered_message_ids,
        covered_through_message_id,
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

fn minimum_summary_budget(source_tokens: u64, policy: &ContextCompactionPolicy) -> u64 {
    if source_tokens == 0 {
        return 0;
    }
    policy.minimum_summary_tokens.min(source_tokens)
}

fn maximum_summary_budget(source_tokens: u64, policy: &ContextCompactionPolicy) -> u64 {
    source_tokens.min(policy.maximum_summary_tokens)
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
        ContextCompactionProtectionReason::UserAttachment => "user_attachment",
        ContextCompactionProtectionReason::RuntimeGuard => "runtime_guard",
        ContextCompactionProtectionReason::VisualInput => "visual_input",
        ContextCompactionProtectionReason::MixedAtomicGroup => "mixed_atomic_group",
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
        planned_durable_reclaimed_tokens: 0,
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
    use crate::protocol::AgentToolApprovalMode;
    use serde_json::json;

    fn planning_item(
        index: usize,
        usage_class: ContextUsageClass,
        estimated_tokens: u64,
        role: LlmMessageRole,
        source: ContextSource,
    ) -> ContextFramePlanningItem {
        ContextFramePlanningItem {
            index,
            usage_class,
            estimated_tokens,
            role,
            sources: vec![source],
            group_id: None,
            tool_names: Vec::new(),
            image_count: 0,
            is_error: false,
            origin: None,
        }
    }

    fn tool_item(
        mut item: ContextFramePlanningItem,
        group_id: &str,
        tool: Option<&str>,
    ) -> ContextFramePlanningItem {
        item.group_id = Some(group_id.to_string());
        item.tool_names = tool.into_iter().map(ToString::to_string).collect();
        item
    }

    fn origin_item(
        mut item: ContextFramePlanningItem,
        origin: ContextOrigin,
    ) -> ContextFramePlanningItem {
        item.origin = Some(origin);
        item
    }

    fn tool(name: &str, safety: AgentToolSafety) -> AgentToolDefinition {
        AgentToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            input_schema: json!({ "type": "object" }),
            safety,
            requires_workspace: false,
            requires_approval: safety != AgentToolSafety::ReadOnly,
            approval_mode: if safety == AgentToolSafety::ReadOnly {
                AgentToolApprovalMode::Never
            } else {
                AgentToolApprovalMode::Always
            },
        }
    }

    fn query(
        status: ContextBudgetStatus,
        available_input_tokens: Option<u64>,
        fixed: u64,
        durable: u64,
        run_transient: u64,
        request_only: u64,
    ) -> ContextCompactionQuery {
        let category = |input_tokens| ContextTokenCategoryEstimate {
            input_tokens,
            ..ContextTokenCategoryEstimate::default()
        };
        let total_tokens = fixed
            .saturating_add(durable)
            .saturating_add(run_transient)
            .saturating_add(request_only);
        ContextCompactionQuery {
            status,
            available_input_tokens,
            remaining_input_tokens: available_input_tokens.map(|available| {
                i64::try_from(available).unwrap_or(i64::MAX)
                    - i64::try_from(total_tokens).unwrap_or(i64::MAX)
            }),
            request_input_tokens: total_tokens,
            additive_input_tokens: total_tokens,
            persistent_input_tokens: fixed.saturating_add(durable),
            context_revision: 7,
            persistent_revision: 5,
            breakdown: ContextTokenBreakdown {
                fixed: category(fixed),
                durable: category(durable),
                run_transient: category(run_transient),
                request_only: category(request_only),
                total: category(total_tokens),
            },
        }
    }

    #[test]
    fn stays_inert_below_the_soft_trigger() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                100,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                300,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
            ),
            planning_item(
                2,
                ContextUsageClass::Durable,
                100,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
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
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::NotRequired);
        assert_eq!(plan.soft_trigger_input_tokens, Some(750));
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn stable_durable_origins_produce_one_complete_message_prefix_boundary() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            origin_item(
                planning_item(
                    1,
                    ContextUsageClass::Durable,
                    500,
                    LlmMessageRole::Assistant,
                    ContextSource::ConversationSummary,
                ),
                ContextOrigin::compaction_summary("summary-previous"),
            ),
            origin_item(
                planning_item(
                    2,
                    ContextUsageClass::Durable,
                    1_500,
                    LlmMessageRole::User,
                    ContextSource::ConversationHistory,
                ),
                ContextOrigin::conversation_message("user-1"),
            ),
            origin_item(
                planning_item(
                    3,
                    ContextUsageClass::Durable,
                    1_000,
                    LlmMessageRole::Assistant,
                    ContextSource::ConversationTrace,
                ),
                ContextOrigin::conversation_message("assistant-1"),
            ),
            origin_item(
                planning_item(
                    4,
                    ContextUsageClass::Durable,
                    2_000,
                    LlmMessageRole::Assistant,
                    ContextSource::ConversationTrace,
                ),
                ContextOrigin::conversation_message("assistant-1"),
            ),
            origin_item(
                planning_item(
                    5,
                    ContextUsageClass::Durable,
                    2_000,
                    LlmMessageRole::Tool,
                    ContextSource::ConversationTrace,
                ),
                ContextOrigin::conversation_message("assistant-1"),
            ),
            origin_item(
                planning_item(
                    6,
                    ContextUsageClass::Durable,
                    1_000,
                    LlmMessageRole::Assistant,
                    ContextSource::ConversationHistory,
                ),
                ContextOrigin::conversation_message("assistant-1"),
            ),
            origin_item(
                planning_item(
                    7,
                    ContextUsageClass::Durable,
                    500,
                    LlmMessageRole::User,
                    ContextSource::CurrentTurn,
                ),
                ContextOrigin::conversation_message("user-current"),
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                8_500,
                0,
                0,
            ),
            &items,
        );

        let step = plan
            .steps
            .iter()
            .find(|step| step.scope == ContextCompactionScope::DurableHistory)
            .unwrap();
        assert_eq!(
            step.ranges,
            vec![ContextCompactionItemRange {
                start_index: 1,
                end_index_exclusive: 7,
            }]
        );
        assert_eq!(step.atomic_unit_count, 3);
        assert_eq!(
            step.durable_prefix,
            Some(ContextCompactionDurablePrefix {
                previous_summary_id: Some("summary-previous".to_string()),
                covered_message_ids: vec!["user-1".to_string(), "assistant-1".to_string()],
                covered_through_message_id: "assistant-1".to_string(),
            })
        );
    }

    #[test]
    fn durable_selection_extends_past_a_large_user_message_to_close_the_turn() {
        let mut items = vec![planning_item(
            0,
            ContextUsageClass::Fixed,
            1_000,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
        )];
        for (index, tokens, role, source, origin) in [
            (
                1,
                500,
                LlmMessageRole::Assistant,
                ContextSource::ConversationSummary,
                ContextOrigin::compaction_summary("summary-previous"),
            ),
            (
                2,
                1_000,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
                ContextOrigin::conversation_message("user-1"),
            ),
            (
                3,
                1_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                ContextOrigin::conversation_message("assistant-1"),
            ),
            (
                4,
                5_500,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
                ContextOrigin::conversation_message("user-2"),
            ),
            (
                5,
                500,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
                ContextOrigin::conversation_message("assistant-2"),
            ),
            (
                6,
                500,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
                ContextOrigin::conversation_message("user-current"),
            ),
        ] {
            items.push(origin_item(
                planning_item(index, ContextUsageClass::Durable, tokens, role, source),
                origin,
            ));
        }

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                9_000,
                0,
                0,
            ),
            &items,
        );

        let step = plan
            .steps
            .iter()
            .find(|step| step.scope == ContextCompactionScope::DurableHistory)
            .unwrap();
        assert_eq!(
            step.ranges,
            vec![ContextCompactionItemRange {
                start_index: 1,
                end_index_exclusive: 6,
            }]
        );
        assert_eq!(
            step.durable_prefix,
            Some(ContextCompactionDurablePrefix {
                previous_summary_id: Some("summary-previous".to_string()),
                covered_message_ids: vec![
                    "user-1".to_string(),
                    "assistant-1".to_string(),
                    "user-2".to_string(),
                    "assistant-2".to_string(),
                ],
                covered_through_message_id: "assistant-2".to_string(),
            })
        );
    }

    #[test]
    fn uses_run_growth_to_choose_a_dynamic_target() {
        let planner = ContextCompactionPlanner::for_tools(&[]);
        let quiet = planner.plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(100_000),
                10_000,
                10_000,
                0,
                60_000,
            ),
            &[],
        );
        let growing = planner.plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(100_000),
                10_000,
                10_000,
                60_000,
                0,
            ),
            &[],
        );

        assert_eq!(quiet.target_input_tokens, Some(75_000));
        assert_eq!(growing.target_input_tokens, Some(70_000));
        assert!(
            growing.required_reclaimed_tokens > quiet.required_reclaimed_tokens,
            "a rapidly growing run should reserve more room for its next tool result"
        );
    }

    #[test]
    fn durable_pressure_targets_fifteen_percent_of_net_capacity() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                10_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                69_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
            ),
            planning_item(
                2,
                ContextUsageClass::Durable,
                1_000,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(100_000),
                10_000,
                70_000,
                0,
                0,
            ),
            &items,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.durable_capacity_tokens, Some(90_000));
        assert_eq!(plan.durable_trigger_input_tokens, Some(67_500));
        assert_eq!(plan.durable_target_input_tokens, Some(13_500));
        assert_eq!(plan.required_durable_reclaimed_tokens, 56_500);
        assert!(plan.projected_durable_input_tokens <= 13_500);
        assert!(plan.projected_durable_input_tokens >= 9_000);
        assert_eq!(plan.protected.reasons["current_user"], 1_000);
        assert!(plan.request_target_satisfied);
        assert!(plan.durable_target_satisfied);
        assert!(!plan.best_effort);
    }

    #[test]
    fn unreachable_durable_target_still_produces_a_best_effort_plan() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                1_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
            ),
            planning_item(
                2,
                ContextUsageClass::Durable,
                8_000,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                9_000,
                0,
                0,
            ),
            &items,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.durable_target_input_tokens, Some(1_350));
        assert_eq!(plan.required_durable_reclaimed_tokens, 7_650);
        assert_eq!(plan.planned_durable_reclaimed_tokens, 744);
        assert_eq!(plan.projected_durable_input_tokens, 8_256);
        assert!(!plan.durable_target_satisfied);
        assert!(plan.best_effort);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.protected.reasons["current_user"], 8_000);
    }

    #[test]
    fn unreachable_durable_target_does_not_block_run_overlay_reclaim() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                1_000,
                LlmMessageRole::Assistant,
                ContextSource::ConversationHistory,
            ),
            planning_item(
                2,
                ContextUsageClass::Durable,
                8_000,
                LlmMessageRole::User,
                ContextSource::CurrentTurn,
            ),
            planning_item(
                3,
                ContextUsageClass::RunTransient,
                10_000,
                LlmMessageRole::Assistant,
                ContextSource::ModelResponse,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::OverBudget,
                Some(10_000),
                1_000,
                9_000,
                10_000,
                0,
            ),
            &items,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert!(plan.best_effort);
        assert!(plan
            .steps
            .iter()
            .any(|step| step.scope == ContextCompactionScope::DurableHistory));
        assert!(plan
            .steps
            .iter()
            .any(|step| step.scope == ContextCompactionScope::RunOverlay));
    }

    #[test]
    fn selects_a_closed_read_tool_exchange_as_one_atomic_range() {
        let tools = vec![
            tool("read_file", AgentToolSafety::ReadOnly),
            tool("write_file", AgentToolSafety::RequiresApproval),
        ];
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                500,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                500,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
            ),
            tool_item(
                planning_item(
                    2,
                    ContextUsageClass::RunTransient,
                    1_000,
                    LlmMessageRole::Assistant,
                    ContextSource::ModelResponse,
                ),
                "read-group",
                Some("read_file"),
            ),
            tool_item(
                planning_item(
                    3,
                    ContextUsageClass::RunTransient,
                    5_000,
                    LlmMessageRole::Tool,
                    ContextSource::ToolResult,
                ),
                "read-group",
                None,
            ),
            tool_item(
                planning_item(
                    4,
                    ContextUsageClass::RunTransient,
                    1_000,
                    LlmMessageRole::Assistant,
                    ContextSource::ModelResponse,
                ),
                "write-group",
                Some("write_file"),
            ),
            tool_item(
                planning_item(
                    5,
                    ContextUsageClass::RunTransient,
                    2_000,
                    LlmMessageRole::Tool,
                    ContextSource::ToolResult,
                ),
                "write-group",
                None,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&tools).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                500,
                500,
                9_000,
                0,
            ),
            &items,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].scope, ContextCompactionScope::RunOverlay);
        assert_eq!(plan.steps[0].atomic_unit_count, 1);
        assert_eq!(plan.steps[0].source_input_tokens, 6_000);
        assert_eq!(plan.steps[0].maximum_summary_tokens, 2_000);
        assert_eq!(plan.steps[0].expected_reclaimed_tokens, 4_000);
        assert_eq!(plan.projected_request_input_tokens, 6_000);
        assert_eq!(
            plan.steps[0].ranges,
            vec![ContextCompactionItemRange {
                start_index: 2,
                end_index_exclusive: 4,
            }]
        );
        assert!(!plan.steps[0].contains_side_effects);
    }

    #[test]
    fn reports_insufficient_context_when_everything_is_protected() {
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            planning_item(
                1,
                ContextUsageClass::Durable,
                9_000,
                LlmMessageRole::User,
                ContextSource::ConversationHistory,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                9_000,
                0,
                0,
            ),
            &items,
        );

        assert_eq!(
            plan.status,
            ContextCompactionPlanStatus::InsufficientCompactableContext
        );
        assert_eq!(plan.compactable_input_tokens, 0);
        assert_eq!(plan.protected.input_tokens, 10_000);
        assert_eq!(plan.protected.reasons["current_user"], 9_000);
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn protects_attachments_and_request_only_context() {
        let mut attachment = planning_item(
            1,
            ContextUsageClass::RunTransient,
            3_000,
            LlmMessageRole::User,
            ContextSource::InputAttachment,
        );
        attachment.image_count = 1;
        let items = vec![
            planning_item(
                0,
                ContextUsageClass::Fixed,
                1_000,
                LlmMessageRole::System,
                ContextSource::BackendSystemPrompt,
            ),
            attachment,
            planning_item(
                2,
                ContextUsageClass::RequestOnly,
                2_000,
                LlmMessageRole::System,
                ContextSource::RuntimeExtension,
            ),
        ];

        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(
                ContextBudgetStatus::OverBudget,
                Some(5_000),
                1_000,
                0,
                3_000,
                2_000,
            ),
            &items,
        );

        assert_eq!(
            plan.status,
            ContextCompactionPlanStatus::InsufficientCompactableContext
        );
        assert_eq!(plan.protected.reasons["user_attachment"], 3_000);
        assert_eq!(plan.protected.reasons["request_only"], 2_000);
    }

    #[test]
    fn policy_constructor_supports_future_configuration_without_changing_planner_contract() {
        let policy = ContextCompactionPolicy {
            soft_trigger_percent: 80,
            ..ContextCompactionPolicy::default()
        };
        let plan = ContextCompactionPlanner::with_policy(policy, &[]).plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(10_000),
                1_000,
                6_500,
                0,
                0,
            ),
            &[],
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::NotRequired);
        assert_eq!(plan.soft_trigger_input_tokens, Some(8_000));
    }
}
