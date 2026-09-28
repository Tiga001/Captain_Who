//! Bounded process-local scheduling hints, never authority to claim or select configuration.
use super::*;
use mycopilot_core::workflow_execution::PendingInputCandidate;

const MAX_RETRIES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetryReason {
    Capacity,
    Busy,
    Configuration,
    Access,
    Transition,
    Unavailable,
    Transient,
}
impl RetryReason {
    fn delay(self, attempts: u32) -> Duration {
        let (base, maximum) = match self {
            Self::Capacity => (1, 1),
            Self::Busy => (1, 8),
            Self::Configuration => (5, 60),
            Self::Access => (10, 60),
            Self::Transition => (1, 15),
            Self::Unavailable => (10, 60),
            Self::Transient => (1, 30),
        };
        Duration::from_secs((base * (1_u64 << attempts.saturating_sub(1).min(6))).min(maximum))
    }
}
#[derive(Debug)]
struct Retry {
    version: String,
    conversation_id: Option<String>,
    reason: RetryReason,
    attempts: u32,
    next: Instant,
    touched: Instant,
    seen_cycle: u64,
}
#[derive(Default, Debug)]
pub(super) struct WorkflowRetryState {
    pub(super) cursor: u64,
    pub(super) generation: u64,
    pub(super) restart_requested: bool,
    pub(super) scanning: bool,
    cycle: u64,
    scan_failures: u32,
    scan_not_before: Option<Instant>,
    entries: HashMap<String, Retry>,
    #[cfg(test)]
    pub(super) attempts: u64,
}
impl WorkflowRetryState {
    pub(super) fn is_due(&mut self, input: &PendingInputCandidate, now: Instant) -> bool {
        let due = self.entries.get_mut(&input.id).is_none_or(|retry| {
            retry.seen_cycle = self.cycle;
            retry.version != input.execution_version || retry.next <= now
        });
        #[cfg(test)]
        if due {
            self.attempts += 1;
        }
        due
    }
    pub(super) fn defer(
        &mut self,
        input: &PendingInputCandidate,
        reason: RetryReason,
        generation: u64,
        now: Instant,
    ) {
        // A condition may change while a blocking readiness check is in flight. Its wake wins:
        // never install a stale cooldown after that condition was invalidated.
        if generation != self.generation {
            return;
        }
        let attempts = self
            .entries
            .get(&input.id)
            .filter(|retry| retry.version == input.execution_version && retry.reason == reason)
            .map_or(1, |retry| retry.attempts.saturating_add(1));
        if self.entries.len() >= MAX_RETRIES && !self.entries.contains_key(&input.id) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, retry)| retry.touched)
                .map(|(id, _)| id.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            input.id.clone(),
            Retry {
                version: input.execution_version.clone(),
                conversation_id: input.conversation_id.clone(),
                reason,
                attempts,
                next: now + reason.delay(attempts),
                touched: now,
                seen_cycle: self.cycle,
            },
        );
    }
    pub(super) fn complete_cycle(&mut self) {
        self.entries
            .retain(|_, retry| retry.seen_cycle == self.cycle);
        self.cycle = self.cycle.wrapping_add(1);
    }
    pub(super) fn scan_failed(&mut self, now: Instant) {
        self.scanning = false;
        self.scan_failures = self.scan_failures.saturating_add(1);
        self.scan_not_before = Some(now + RetryReason::Transient.delay(self.scan_failures));
    }
    pub(super) fn scan_succeeded(&mut self) {
        self.scan_failures = 0;
        self.scan_not_before = None;
    }
    pub(super) fn next_delay(&self, now: Instant) -> Option<Duration> {
        if let Some(deadline) = self.scan_not_before {
            return Some(
                deadline
                    .saturating_duration_since(now)
                    .max(Duration::from_millis(10)),
            );
        }
        self.entries
            .values()
            .map(|retry| retry.next.saturating_duration_since(now))
            .min()
            .map(|delay| delay.max(Duration::from_millis(10)))
    }
    pub(super) fn forget(&mut self, input_id: &str) {
        self.entries.remove(input_id);
    }
    pub(super) fn capacity_changed(&mut self) {
        self.entries
            .retain(|_, retry| retry.reason != RetryReason::Capacity);
        self.restart_requested |= self.cursor != 0 || self.scanning;
        self.generation = self.generation.wrapping_add(1);
    }
    pub(super) fn changed(&mut self, conversation_id: Option<&str>) {
        self.scan_succeeded();
        match conversation_id {
            Some(id) => self
                .entries
                .retain(|_, retry| retry.conversation_id.as_deref() != Some(id)),
            None => self.entries.clear(),
        }
        self.restart_requested |= self.cursor != 0 || self.scanning;
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(id: &str) -> PendingInputCandidate {
        PendingInputCandidate {
            id: id.into(),
            sequence: 1,
            instance_id: "workflow".into(),
            execution_version: "v1".into(),
            conversation_id: Some(id.into()),
        }
    }
    #[test]
    fn retry_is_bounded_by_reason_and_cleared_by_version_or_exact_condition() {
        let mut state = WorkflowRetryState::default();
        let now = Instant::now();
        let a = input("a");
        let b = input("b");
        state.defer(&a, RetryReason::Configuration, 0, now);
        state.defer(&b, RetryReason::Busy, 0, now);
        assert!(!state.is_due(&a, now + Duration::from_secs(4)));
        assert!(state.is_due(&b, now + Duration::from_secs(1)));
        state.changed(Some("a"));
        assert!(state.is_due(&a, now));
        assert!(!state.is_due(&b, now));
        state.defer(&a, RetryReason::Configuration, 1, now);
        let mut newer = a.clone();
        newer.execution_version = "v2".into();
        assert!(state.is_due(&newer, now));
        for reason in [
            RetryReason::Capacity,
            RetryReason::Busy,
            RetryReason::Configuration,
            RetryReason::Access,
            RetryReason::Transition,
            RetryReason::Unavailable,
            RetryReason::Transient,
        ] {
            assert!(reason.delay(u32::MAX) <= Duration::from_secs(60));
        }
        for id in 0..MAX_RETRIES + 10 {
            state.defer(&input(&id.to_string()), RetryReason::Busy, 1, now);
        }
        assert_eq!(state.entries.len(), MAX_RETRIES);
        state.changed(None);
        assert!(state.entries.is_empty());
    }
    #[test]
    fn condition_change_during_check_cannot_be_lost_to_new_cooldown() {
        let mut state = WorkflowRetryState::default();
        let generation = state.generation;
        state.scanning = true; // A change during the first page also needs a wraparound pass.
        state.changed(Some("a"));
        assert!(state.restart_requested);
        state.defer(
            &input("a"),
            RetryReason::Configuration,
            generation,
            Instant::now(),
        );
        assert!(state.is_due(&input("a"), Instant::now()));
    }
}
