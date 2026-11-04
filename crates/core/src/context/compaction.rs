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

const COMPACTION_TRIGGER_PERCENT: u64 = 90;
const RECENT_CONTEXT_TARGET_TOKENS: u64 = 24_000;
const SUMMARY_TARGET_TOKENS: u64 = 12_000;

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
    WorldState,
    UserAttachment,
    RuntimeGuard,
    VisualInput,
    MixedAtomicGroup,
    UncommittedRun,
    CompactionRetained,
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
    /// Exact user, active Run material, and historical visual tokens retained beside the summary.
    pub(crate) retained_input_tokens: u64,
    /// Aspirational size of the model-visible semantic summary. Backend-only Continuity metadata
    /// is deliberately excluded. This is a planning projection, not an output limit.
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
    pub(crate) required_reclaimed_tokens: u64,
    pub(crate) planned_reclaimed_tokens: u64,
    pub(crate) projected_request_input_tokens: u64,
    pub(crate) request_target_satisfied: bool,
    pub(crate) best_effort: bool,
    pub(crate) compactable_input_tokens: u64,
    pub(crate) protected: ContextCompactionProtectedEstimate,
    pub(crate) steps: Vec<ContextCompactionStep>,
}

pub(crate) struct ContextCompactionPlanner {
    tool_safety: BTreeMap<String, AgentToolSafety>,
}

impl ContextCompactionPlanner {
    pub(crate) fn for_tools(tools: &[AgentToolDefinition]) -> Self {
        Self {
            tool_safety: tools
                .iter()
                .map(|tool| (tool.name.clone(), tool.safety))
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn plan(
        &self,
        query: &ContextCompactionQuery,
        items: &[ContextFramePlanningItem],
        protect_current_user: bool,
    ) -> ContextCompactionPlan {
        self.plan_with_headroom(query, items, protect_current_user, 0, false)
    }

    /// `probe` selects the same complete prefix below the soft threshold so the Host can
    /// measure the actual summary request before it becomes too large. It never splits a turn
    /// or initiates model work. Headroom accounts for growth between safe request boundaries.
    pub(crate) fn plan_with_headroom(
        &self,
        query: &ContextCompactionQuery,
        items: &[ContextFramePlanningItem],
        protect_current_user: bool,
        growth_headroom: u64,
        probe: bool,
    ) -> ContextCompactionPlan {
        let units = build_atomic_units(items, &self.tool_safety);
        let latest_user_index = items
            .iter()
            .rev()
            .find(|item| {
                item.role == LlmMessageRole::User
                    && !item.sources.contains(&ContextSource::HistoricalRunContext)
                    && !(is_trace_origin(item.origin.as_ref())
                        && item.sources.iter().copied().any(is_run_material_source))
            })
            .map(|item| item.index);
        let mut protected_reasons = BTreeMap::<String, u64>::new();
        let mut protected_unit_count = 0_usize;
        let mut candidates = Vec::new();
        for unit in units.iter().cloned() {
            let contains_latest_user = protect_current_user
                && latest_user_index.is_some_and(|index| {
                    unit.start_index <= index && index < unit.end_index_exclusive
                });
            // The latest durable user message is a journal bridge: storage may summarize through
            // it to reach later closed activity, but projection restores the exact message after
            // the summary. Run-scoped guidance is instead a hard boundary because its attachments
            // and placement are part of the live run.
            // Historical visual material is also an exact journal bridge. Its original text
            // and image references are restored after coverage, rather than making every later
            // textual history entry permanently unreachable to prefix compaction.
            let retained_visual_material =
                is_historical_run_material(&unit) && unit.contains_images;
            // Once active Run material has a journal identity it is included in the exact raw
            // source prefix too. Count it on both sides of replacement instead of omitting it
            // from the measured source or dropping its live projection after compaction.
            let retained_active_material = !unit.mixed_usage_classes
                && !unit.sources.contains(&ContextSource::CompactionRetained)
                && unit.usage_class == ContextUsageClass::RunTransient
                && is_trace_origin(unit.origin.as_ref())
                && unit.sources.iter().copied().any(is_run_material_source);
            let retained_after_compaction = (contains_latest_user
                && unit.usage_class == ContextUsageClass::Durable)
                || retained_visual_material
                || retained_active_material;
            if contains_latest_user && !retained_after_compaction {
                protected_unit_count = protected_unit_count.saturating_add(1);
                merge_reason_tokens(
                    &mut protected_reasons,
                    ContextCompactionProtectionReason::CurrentUser,
                    unit.tokens,
                );
                continue;
            }
            if let Some(reason) = absolute_protection_reason(&unit) {
                if !retained_active_material {
                    protected_unit_count = protected_unit_count.saturating_add(1);
                    merge_reason_tokens(&mut protected_reasons, reason, unit.tokens);
                    continue;
                }
            }
            match unit.usage_class {
                ContextUsageClass::Durable | ContextUsageClass::RunTransient => {}
                ContextUsageClass::Fixed | ContextUsageClass::RequestOnly => {
                    unreachable!("fixed and request-only units are protected above")
                }
            }
            if unit.origin.is_none() {
                protected_unit_count = protected_unit_count.saturating_add(1);
                merge_reason_tokens(
                    &mut protected_reasons,
                    ContextCompactionProtectionReason::UncommittedRun,
                    unit.tokens,
                );
                continue;
            }
            if retained_after_compaction {
                protected_unit_count = protected_unit_count.saturating_add(1);
                merge_reason_tokens(
                    &mut protected_reasons,
                    if retained_visual_material {
                        ContextCompactionProtectionReason::VisualInput
                    } else if retained_active_material {
                        absolute_protection_reason(&unit)
                            .expect("active Run material has an exact protection reason")
                    } else {
                        ContextCompactionProtectionReason::CurrentUser
                    },
                    unit.tokens,
                );
            }
            candidates.push(CompactionCandidate {
                unit,
                retained_after_compaction,
            });
        }

        let compactable_input_tokens = candidates
            .iter()
            .filter(|candidate| !candidate.retained_after_compaction)
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
            percent_ceil(available_input_tokens, COMPACTION_TRIGGER_PERCENT)
                .min(available_input_tokens.saturating_sub(growth_headroom));
        // Recent evidence and the summary have independent budgets. A fixed percentage of
        // the entire window used to squeeze a 400K history into a ~3K summary when protected
        // material occupied most of the target. Scale down only for small context windows.
        let recent_target = RECENT_CONTEXT_TARGET_TOKENS.min(available_input_tokens / 5);
        let summary_target = SUMMARY_TARGET_TOKENS
            .min(available_input_tokens / 10)
            .max(1);
        let target_input_tokens = protected
            .input_tokens
            .saturating_add(recent_target)
            .saturating_add(summary_target)
            // A growth spike advances the trigger, but must not also erase the recent-history
            // and summary targets. Leave at least 20% working room after a successful summary.
            .min(available_input_tokens.saturating_sub(available_input_tokens / 5));
        let thresholds = ContextCompactionThresholds {
            request_trigger_input_tokens: Some(soft_trigger_input_tokens),
            request_target_input_tokens: Some(target_input_tokens),
        };
        let request_pressure = probe
            || query.request_input_tokens >= soft_trigger_input_tokens
            || query.status == ContextBudgetStatus::OverBudget;
        if !request_pressure {
            return empty_plan(
                ContextCompactionPlanStatus::NotRequired,
                query,
                thresholds,
                compactable_input_tokens,
                protected,
            );
        }

        let required_reclaimed_tokens = query
            .request_input_tokens
            .saturating_sub(target_input_tokens);

        let mut selected = Vec::<CompactionCandidate>::new();
        for candidate in &candidates {
            if maximum_reclaimable_tokens(&selected)
                >= required_reclaimed_tokens.saturating_add(summary_target)
            {
                break;
            }
            selected.push(candidate.clone());
        }
        normalize_unified_journal_prefix(&mut selected, &units, &candidates);

        let steps = build_steps(&selected, required_reclaimed_tokens, summary_target);
        let planned_reclaimed_tokens = steps
            .iter()
            .map(|step| step.expected_reclaimed_tokens)
            .sum::<u64>();
        let projected_request_input_tokens = query
            .request_input_tokens
            .saturating_sub(planned_reclaimed_tokens);
        let request_target_satisfied = projected_request_input_tokens <= target_input_tokens;
        let best_effort = !request_target_satisfied;
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
            required_reclaimed_tokens,
            planned_reclaimed_tokens,
            projected_request_input_tokens,
            request_target_satisfied,
            best_effort,
            compactable_input_tokens,
            protected,
            steps,
        }
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
    retained_after_compaction: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct ContextCompactionThresholds {
    request_trigger_input_tokens: Option<u64>,
    request_target_input_tokens: Option<u64>,
}

impl ContextCompactionThresholds {
    fn zero() -> Self {
        Self {
            request_trigger_input_tokens: Some(0),
            request_target_input_tokens: Some(0),
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
        let end = match first.group_id.as_deref() {
            Some(group_id) => items[cursor..]
                .iter()
                .take_while(|item| item.group_id.as_deref() == Some(group_id))
                .count()
                .saturating_add(cursor),
            None => match (
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
                _ => cursor.saturating_add(1),
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
            // A closed atomic exchange is covered through its final journal record (normally the
            // ToolResult), never through the opening ToolCall.
            origin: slice.iter().rev().find_map(|item| item.origin.clone()),
            first_role: first.role,
            last_role: slice.last().map_or(first.role, |item| item.role),
        });
        cursor = end;
    }
    units
}

fn absolute_protection_reason(
    unit: &AtomicContextUnit,
) -> Option<ContextCompactionProtectionReason> {
    if unit.mixed_usage_classes {
        return Some(ContextCompactionProtectionReason::MixedAtomicGroup);
    }
    if unit.sources.contains(&ContextSource::CompactionRetained) {
        return Some(ContextCompactionProtectionReason::CompactionRetained);
    }
    // Finished-run context is ordinary durable history, even when its original source tag is
    // retained for token attribution. It is not a live Skill, permission or attachment overlay.
    // Visual historical material is admitted as a retained bridge by the planner above.
    if is_historical_run_material(unit) {
        return None;
    }
    // Activated Skill instructions are an immutable run snapshot. They must survive every
    // compaction attempt exactly as selected, even if run-transient policy is refined later.
    if unit.sources.contains(&ContextSource::SkillInstructions) {
        return Some(ContextCompactionProtectionReason::SkillInstructions);
    }
    if unit.sources.contains(&ContextSource::WorldStateSnapshot)
        || unit.sources.contains(&ContextSource::WorldStateDiff)
    {
        return Some(ContextCompactionProtectionReason::WorldState);
    }
    match unit.usage_class {
        ContextUsageClass::Fixed => return Some(ContextCompactionProtectionReason::FixedRequest),
        ContextUsageClass::RequestOnly => {
            return Some(ContextCompactionProtectionReason::RequestOnly)
        }
        ContextUsageClass::RunTransient | ContextUsageClass::Durable => {}
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

fn is_historical_run_material(unit: &AtomicContextUnit) -> bool {
    unit.usage_class == ContextUsageClass::Durable
        && unit.sources.contains(&ContextSource::HistoricalRunContext)
}

fn is_trace_origin(origin: Option<&ContextOrigin>) -> bool {
    origin.is_some_and(|origin| origin.kind() == ContextOriginKind::ConversationTraceItem)
}

fn is_run_material_source(source: ContextSource) -> bool {
    matches!(
        source,
        ContextSource::SkillInstructions
            | ContextSource::InputAttachment
            | ContextSource::WorldStateSnapshot
            | ContextSource::WorldStateDiff
    )
}

fn is_journal_run_material(unit: &AtomicContextUnit) -> bool {
    is_historical_run_material(unit)
        || (is_trace_origin(unit.origin.as_ref())
            && unit.sources.iter().copied().any(is_run_material_source))
}

fn maximum_reclaimable_tokens(selected: &[CompactionCandidate]) -> u64 {
    // The target is deliberately soft. Prefix selection therefore uses the theoretical maximum
    // reclaim and leaves feasibility to the executor, which validates the measured semantic
    // summary replacement against the source.
    selected
        .iter()
        .filter(|candidate| !candidate.retained_after_compaction)
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>()
}

fn build_steps(
    selected: &[CompactionCandidate],
    required_reclaimed_tokens: u64,
    summary_target: u64,
) -> Vec<ContextCompactionStep> {
    if selected.is_empty() || required_reclaimed_tokens == 0 {
        return Vec::new();
    }
    let mut candidates = selected.iter().collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.unit.start_index);
    let source_input_tokens = candidates
        .iter()
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>();
    let retained_input_tokens = candidates
        .iter()
        .filter(|candidate| candidate.retained_after_compaction)
        .map(|candidate| candidate.unit.tokens)
        .sum::<u64>();
    let reclaimable_input_tokens = source_input_tokens.saturating_sub(retained_input_tokens);
    // The exact latest user instruction is restored beside the summary after commit, so the
    // summary budget excludes those retained tokens. It is independent of how close the
    // protected request remainder happens to be to the post-compaction target.
    let target_replacement_tokens = summary_target.min(reclaimable_input_tokens / 4);
    let expected_reclaimed_tokens =
        reclaimable_input_tokens.saturating_sub(target_replacement_tokens);
    if expected_reclaimed_tokens == 0 {
        return Vec::new();
    }
    vec![ContextCompactionStep {
        ranges: merge_candidate_ranges(&candidates),
        atomic_unit_count: candidates.len(),
        source_input_tokens,
        retained_input_tokens,
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

fn normalize_unified_journal_prefix(
    selected: &mut Vec<CompactionCandidate>,
    units: &[AtomicContextUnit],
    candidates: &[CompactionCandidate],
) {
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
    for unit in units {
        // These are exact side ledgers or request contracts rather than entries in the
        // conversation message/trace journal. They survive compaction independently and do not
        // interrupt chronological prefix selection.
        if unit.sources.contains(&ContextSource::CompactionRetained)
            || (!is_journal_run_material(unit)
                && (unit.sources.contains(&ContextSource::WorldStateSnapshot)
                    || unit.sources.contains(&ContextSource::WorldStateDiff)
                    || unit.sources.contains(&ContextSource::SkillInstructions)
                    || unit.sources.contains(&ContextSource::SkillCatalog)
                    || unit.sources.contains(&ContextSource::RuntimeTodo)
                    || unit.sources.contains(&ContextSource::FileTransaction)
                    || unit.sources.contains(&ContextSource::RuntimeGuard)
                    || unit.sources.contains(&ContextSource::InputAttachment)
                    || (unit.origin.is_none()
                        && matches!(
                            unit.usage_class,
                            ContextUsageClass::Fixed | ContextUsageClass::RequestOnly
                        ))))
        {
            continue;
        }
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
                if !compaction_summary_roles_are_valid(unit.first_role, unit.last_role) {
                    return None;
                }
            }
            ContextOriginKind::ConversationMessage | ContextOriginKind::ConversationTraceItem => {
                covered_through = origin.journal_cursor();
            }
            ContextOriginKind::WorldStateRecord | ContextOriginKind::Skill => return None,
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

fn compaction_summary_roles_are_valid(
    first_role: LlmMessageRole,
    last_role: LlmMessageRole,
) -> bool {
    matches!(
        (first_role, last_role),
        (LlmMessageRole::Assistant, LlmMessageRole::Assistant)
            | (LlmMessageRole::System, LlmMessageRole::System)
    )
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
        ContextCompactionProtectionReason::WorldState => "world_state",
        ContextCompactionProtectionReason::UserAttachment => "user_attachment",
        ContextCompactionProtectionReason::RuntimeGuard => "runtime_guard",
        ContextCompactionProtectionReason::VisualInput => "visual_input",
        ContextCompactionProtectionReason::MixedAtomicGroup => "mixed_atomic_group",
        ContextCompactionProtectionReason::UncommittedRun => "uncommitted_run",
        ContextCompactionProtectionReason::CompactionRetained => "compaction_retained",
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
        required_reclaimed_tokens: 0,
        planned_reclaimed_tokens: 0,
        projected_request_input_tokens: query.request_input_tokens,
        request_target_satisfied: status == ContextCompactionPlanStatus::NotRequired,
        best_effort: false,
        compactable_input_tokens,
        protected,
        steps: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
