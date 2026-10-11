//! Bounded process-local ownership of child Runtime cancellation and final-fence refusals.
//! SQLite still owns recovery. A refusal only prevents this process from keeping the rejected
//! generation alive; it never terminalizes a Turn or releases its durable logical permit.
use super::*;
use mycopilot_core::TrustedAgentWakeTurnAdmission;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ExecutionIdentity {
    agent_id: String,
    wake_id: String,
    claim_token: String,
    run_id: String,
    assistant_message_id: String,
}

impl ExecutionIdentity {
    fn new(trusted: &TrustedAgentWakeTurnAdmission, run: &str, assistant: &str) -> Self {
        Self {
            agent_id: trusted.agent_id.clone(),
            wake_id: trusted.wake_id.clone(),
            claim_token: trusted.claim_token.clone(),
            run_id: run.into(),
            assistant_message_id: assistant.into(),
        }
    }
}

struct ExecutionOwner {
    cancellation: Option<AgentCancellationToken>,
    refused: bool,
}

pub(super) struct WakeExecutionFences {
    owners: HashMap<ExecutionIdentity, ExecutionOwner>,
    capacity: usize,
}

impl WakeExecutionFences {
    pub(super) fn new(logical_limit: usize) -> Self {
        Self {
            owners: HashMap::new(),
            capacity: logical_limit,
        }
    }

    fn upsert(
        &mut self,
        identity: ExecutionIdentity,
        cancellation: Option<AgentCancellationToken>,
        refused: bool,
    ) -> Result<(), String> {
        if !self.owners.contains_key(&identity) && self.owners.len() >= self.capacity {
            return Err("child Runtime ownership registry reached the logical Turn limit; durable recovery required".into());
        }
        let owner = self.owners.entry(identity).or_insert(ExecutionOwner {
            cancellation: None,
            refused: false,
        });
        if !refused && owner.refused {
            return Err(
                "this child Runtime claim was already refused; durable recovery required".into(),
            );
        }
        if cancellation.is_some() {
            owner.cancellation = cancellation;
        }
        owner.refused |= refused;
        Ok(())
    }

    fn cancel_owned(&self, wake: &str, claim: &str, run: &str, assistant: &str) {
        if let Some(owner) = self.owners.iter().find_map(|(identity, owner)| {
            (identity.wake_id == wake
                && identity.claim_token == claim
                && identity.run_id == run
                && identity.assistant_message_id == assistant)
                .then_some(owner)
        }) {
            if let Some(token) = &owner.cancellation {
                token.cancel();
            }
        }
    }

    fn claim_refused(&self, wake: &str, claim: &str) -> bool {
        self.owners.iter().any(|(identity, owner)| {
            identity.wake_id == wake && identity.claim_token == claim && owner.refused
        })
    }
}

impl AgentService {
    /// Called only by bounded blocking preparation/completion workers. Never hold the registry
    /// across an async wait. Cleanup reads SQLite without holding the registry, so renewal may
    /// serialize its small write under the registry without introducing a DB/registry inversion.
    pub(crate) fn reconcile_agent_wake_execution_fences(&self) -> Result<(), String> {
        self.prune_agent_wake_execution_fences(false)
    }

    fn prune_agent_wake_execution_fences(&self, include_running: bool) -> Result<(), String> {
        let identities: Vec<_> = self
            .wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .owners
            .iter()
            .filter(|(_, owner)| include_running || owner.refused)
            .map(|(identity, _)| identity.clone())
            .collect();
        for identity in identities {
            let current = self
                .storage
                .get_agent_wake(&identity.wake_id)
                .map_err(|error| error.to_string())?;
            let retain = current.is_some_and(|wake| {
                !wake.status.is_terminal()
                    && wake.agent_id == identity.agent_id
                    && wake.claim_token.as_deref() == Some(identity.claim_token.as_str())
                    && wake.run_id.as_deref() == Some(identity.run_id.as_str())
                    && wake.assistant_message_id.as_deref()
                        == Some(identity.assistant_message_id.as_str())
            });
            if !retain {
                if let Some(owner) = self
                    .wake_execution_fences
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .owners
                    .remove(&identity)
                {
                    if let Some(cancellation) = owner.cancellation {
                        cancellation.cancel();
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn register_agent_wake_runtime_owner(
        &self,
        trusted: &TrustedAgentWakeTurnAdmission,
        run: &str,
        assistant: &str,
        cancellation: &AgentCancellationToken,
    ) -> Result<(), String> {
        let needs_cleanup = {
            let owners = self
                .wake_execution_fences
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            owners.owners.len() >= owners.capacity
                || owners.owners.keys().any(|identity| {
                    identity.wake_id == trusted.wake_id
                        && identity.claim_token != trusted.claim_token
                })
        };
        // Ordinary registration is memory-only. SQLite pruning is paid only on capacity or
        // generation replacement; recovery scans separately prune the uncommon refused set.
        if needs_cleanup {
            self.prune_agent_wake_execution_fences(true)?;
        }
        self.wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .upsert(
                ExecutionIdentity::new(trusted, run, assistant),
                Some(cancellation.clone()),
                false,
            )
    }

    pub(crate) fn record_fenced_agent_wake_execution(
        &self,
        trusted: &TrustedAgentWakeTurnAdmission,
        run: &str,
        assistant: &str,
    ) -> Result<(), String> {
        // Existing entries must be refused even if SQLite is temporarily unavailable. New
        // entries only arise before registration completed and still own a logical Turn slot.
        let identity = ExecutionIdentity::new(trusted, run, assistant);
        let result = self
            .wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .upsert(identity, None, true);
        self.notify_durable_turn_observers(assistant);
        result
    }

    pub(crate) fn retire_agent_wake_runtime_owner(
        &self,
        trusted: &TrustedAgentWakeTurnAdmission,
        run: &str,
        assistant: &str,
        cancellation: &AgentCancellationToken,
    ) {
        let identity = ExecutionIdentity::new(trusted, run, assistant);
        let mut owners = self
            .wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owners.owners.get(&identity).is_some_and(|owner| {
            !owner.refused
                && owner
                    .cancellation
                    .as_ref()
                    .is_some_and(|current| current.shares_state_with(cancellation))
        }) {
            owners.owners.remove(&identity);
        }
    }

    pub(crate) fn agent_wake_execution_refused(
        &self,
        wake: &str,
        claim: &str,
        run: &str,
        assistant: &str,
    ) -> bool {
        self.wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .owners
            .iter()
            .any(|(identity, owner)| {
                owner.refused
                    && identity.wake_id == wake
                    && identity.claim_token == claim
                    && identity.run_id == run
                    && identity.assistant_message_id == assistant
            })
    }

    pub(crate) fn renew_unfenced_agent_wake_lease(
        &self,
        wake: &str,
        claim: &str,
        renew: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let owners = self
            .wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if owners.claim_refused(wake, claim) {
            return Err(
                "child Runtime refused this claim; lease renewal stopped for durable recovery"
                    .into(),
            );
        }
        // Refusal publication and renewal are serialized. A renewal already holding this lock
        // finishes before refusal is recorded; no renewal begins after a recorded refusal.
        renew()
    }

    pub(crate) fn cancel_owned_agent_wake_runtime(
        &self,
        wake: &str,
        claim: &str,
        run: &str,
        assistant: &str,
    ) {
        self.wake_execution_fences
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancel_owned(wake, claim, run, assistant);
        // Never cancel by run_id here: a replacement generation may reuse that durable ID.
        // Missing local ownership is left to the final fresh fence / durable recovery protocol.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(claim: &str) -> ExecutionIdentity {
        ExecutionIdentity {
            agent_id: "agent".into(),
            wake_id: "wake".into(),
            claim_token: claim.into(),
            run_id: "same-run".into(),
            assistant_message_id: "same-assistant".into(),
        }
    }
    #[test]
    fn refusal_is_generation_specific_and_cannot_be_reopened() {
        let mut registry = WakeExecutionFences::new(2);
        registry
            .upsert(identity("old"), Some(AgentCancellationToken::new()), false)
            .unwrap();
        registry.upsert(identity("old"), None, true).unwrap();
        assert!(registry.claim_refused("wake", "old"));
        assert!(registry
            .upsert(identity("old"), Some(AgentCancellationToken::new()), false)
            .is_err());
        registry
            .upsert(identity("new"), Some(AgentCancellationToken::new()), false)
            .unwrap();
        assert!(!registry.claim_refused("wake", "new"));
        assert!(!registry.claim_refused("other-wake", "old"));
    }
    #[test]
    fn cancellation_targets_exact_generation_even_when_run_identity_is_reused() {
        let mut registry = WakeExecutionFences::new(2);
        let old = AgentCancellationToken::new();
        let successor = AgentCancellationToken::new();
        registry
            .upsert(identity("old"), Some(old.clone()), false)
            .unwrap();
        registry
            .upsert(identity("new"), Some(successor.clone()), false)
            .unwrap();
        registry.cancel_owned("wake", "old", "same-run", "same-assistant");
        assert!(old.is_cancelled());
        assert!(!successor.is_cancelled());
        registry.cancel_owned("wake", "missing", "same-run", "same-assistant");
        assert!(!successor.is_cancelled());
    }
    #[test]
    fn refusal_registry_is_bounded_and_existing_refusal_survives_capacity() {
        let mut registry = WakeExecutionFences::new(1);
        registry.upsert(identity("old"), None, false).unwrap();
        assert!(registry.upsert(identity("new"), None, false).is_err());
        registry.upsert(identity("old"), None, true).unwrap();
        assert!(registry.claim_refused("wake", "old"));
        registry.owners.remove(&identity("old"));
        registry.upsert(identity("new"), None, false).unwrap();
    }
}
