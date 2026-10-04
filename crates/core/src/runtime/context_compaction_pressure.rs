//! Predictive headroom supplements, but never relaxes, the complete request capacity gate.

use crate::{AgentContextWindowSnapshot, AgentError};

#[derive(Default)]
pub(super) struct CompactionPressure {
    previous_request_tokens: Option<u64>,
    previous_summary_tokens: Option<u64>,
    deferred: bool,
}

impl CompactionPressure {
    pub(super) fn request_headroom(&mut self, tokens: u64, capacity: Option<u64>) -> u64 {
        let growth = self
            .previous_request_tokens
            .map_or(0, |old| tokens.saturating_sub(old));
        self.previous_request_tokens = Some(tokens);
        headroom(capacity, growth)
    }

    pub(super) fn summary_pressure(&mut self, budget: &AgentContextWindowSnapshot) -> bool {
        let growth = self
            .previous_summary_tokens
            .map_or(0, |old| budget.input_tokens.saturating_sub(old));
        self.previous_summary_tokens = Some(budget.input_tokens);
        budget.input_capacity_tokens.is_some_and(|capacity| {
            budget
                .input_tokens
                .saturating_add(headroom(Some(capacity), growth))
                >= capacity
        })
    }

    pub(super) fn should_attempt(&self, hard_limit_reached: bool) -> bool {
        !self.deferred || hard_limit_reached
    }
    pub(super) fn defer(&mut self) {
        self.deferred = true;
    }
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

fn headroom(capacity: Option<u64>, growth: u64) -> u64 {
    // Two ordinary 10K tool results are the initial batch allowance. Small windows scale it
    // down; larger observed batches (including mailbox deliveries) advance the next trigger.
    let baseline = capacity.map_or(20_000, |capacity| (capacity / 10).min(20_000));
    baseline.max(growth.saturating_mul(2))
}

pub(super) fn can_defer_compaction_error(error: &AgentError) -> bool {
    // Only a model-generation failure can be deferred. Persistence, projection and commit
    // errors must stop the run even if the old request would still fit.
    if !error
        .model_request_observation()
        .is_some_and(|observation| {
            observation.purpose == crate::ModelRequestPurpose::ContextCompaction
        })
    {
        return false;
    }
    (error.code() == Some("agent.llm_provider_failure")
        && error
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(serde_json::Value::as_bool)
            == Some(true))
        || matches!(
            error.code(),
            Some(
                "context_compaction_empty_summary"
                    | "context_compaction_incomplete_summary"
                    | "context_compaction_not_smaller"
            )
        )
}

pub(super) fn is_context_capacity_rejection(error: &AgentError) -> bool {
    error.code() == Some("agent.llm_provider_failure")
        && error.partial_response().is_none()
        && error
            .details()
            .and_then(|details| details.get("category"))
            .and_then(serde_json::Value::as_str)
            == Some("context_too_large")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_growth_advances_trigger_and_shrinking_history_does_not_inflate_it() {
        let mut pressure = CompactionPressure::default();
        assert_eq!(pressure.request_headroom(400_000, Some(540_000)), 20_000);
        assert_eq!(pressure.request_headroom(445_000, Some(540_000)), 90_000);
        assert_eq!(pressure.request_headroom(80_000, Some(540_000)), 20_000);
        assert_eq!(pressure.request_headroom(100, Some(1_000)), 100);
    }

    #[test]
    fn transient_failure_cooldown_yields_to_capacity_pressure() {
        let mut pressure = CompactionPressure::default();
        assert!(pressure.should_attempt(false));
        pressure.defer();
        assert!(!pressure.should_attempt(false));
        assert!(pressure.should_attempt(true));
        pressure.reset();
        assert!(pressure.should_attempt(false));
        assert!(!can_defer_compaction_error(&AgentError::new(
            "storage failed"
        )));
    }

    #[test]
    fn provider_recovery_only_admits_context_rejection_without_partial_output() {
        let error = AgentError::structured(
            "agent.llm_provider_failure",
            "too large",
            serde_json::json!({"category": "context_too_large", "retryable": false}),
        );
        assert!(is_context_capacity_rejection(&error));
        assert!(!is_context_capacity_rejection(
            &error.with_partial_response("already streamed")
        ));
        assert!(!is_context_capacity_rejection(&AgentError::structured(
            "agent.llm_provider_failure",
            "network",
            serde_json::json!({"category": "network", "retryable": true})
        )));
        assert!(!is_context_capacity_rejection(&AgentError::new(
            "context_too_large"
        )));
    }
}
