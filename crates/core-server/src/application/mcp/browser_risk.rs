//! Process-only browser destination approval coordinator.
//!
//! A managed Browser Tool remains in flight while Electron asks Core to authorize a classified
//! network boundary. This coordinator deliberately does not use the durable Agent continuation
//! machinery: the original request is held unsent by Main and one approval settles its oneshot
//! waiter directly. No BrowserRiskGrant or Host-only DNS fingerprint is persisted.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mycopilot_core::{
    AgentApprovalStatus, AgentBrowserRiskApproval, AgentBuiltinExecutionPermission,
    AgentChatOutput, AgentPermissions, AgentProposedAction, AgentRunStatus,
    BrowserDestinationIdentity, BrowserResolvedAddressClass, BrowserRiskAuthorizationRequest,
    BrowserRiskKind, BrowserRiskTrigger, BuiltinCapabilityId, BuiltinCapabilityRuntime,
    CapabilityActivationId, BROWSER_RISK_APPROVAL_TTL_SECONDS,
};
use mycopilot_protocol_rs::{
    BrowserResolvedAddressClassDto, BrowserRiskAuthorizationDecisionDto, BrowserRiskAuthorizeInput,
    BrowserRiskAuthorizeOutput, BrowserRiskCancelInput, BrowserRiskCancelOutput,
    BrowserRiskKindDto, BrowserRiskTriggerDto, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
};
use serde_json::json;
use tokio::sync::oneshot;
use uuid::{Uuid, Version};

use crate::application::agent::{AgentActionExecutionOutput, CoreServerNotificationSender};
use crate::application::agent_support::{PendingActionStatus, PendingAgentActionSnapshot};

const MAX_PENDING_BROWSER_RISK_APPROVALS: usize = 64;
const MAX_SAFE_REASON_BYTES: usize = 1_024;
const MAX_CALL_ID_BYTES: usize = 512;

type BrowserRiskClock = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BrowserRiskScope {
    run_id: String,
    capability_activation_id: String,
    manifest_digest: String,
    policy_revision: u64,
    origin: String,
    scheme: String,
    ascii_host: String,
    effective_port: u16,
    address_class: BrowserResolvedAddressClass,
    resolution_fingerprint: String,
    risk_kinds: Vec<BrowserRiskKind>,
    /// High-risk actions are scoped to the exact frozen destination/action. Ordinary network
    /// approvals intentionally omit these fields so a stable same-origin grant can be reused.
    exact_target_fingerprint: Option<String>,
    exact_trigger: Option<BrowserRiskTrigger>,
    exact_trigger_tool_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BrowserRiskDenialScope {
    grant_scope: BrowserRiskScope,
    normalized_url: String,
    target_fingerprint: String,
    trigger: BrowserRiskTrigger,
    trigger_tool_name: String,
}

impl BrowserRiskDenialScope {
    fn from_request(request: &BrowserRiskAuthorizationRequest) -> Self {
        Self {
            grant_scope: BrowserRiskScope::from_request(request),
            normalized_url: request.destination.normalized_url.clone(),
            target_fingerprint: request.target_fingerprint.clone(),
            trigger: request.trigger,
            trigger_tool_name: request.trigger_tool_name.clone(),
        }
    }
}

impl BrowserRiskScope {
    fn from_request(request: &BrowserRiskAuthorizationRequest) -> Self {
        let exact = requires_exact_browser_risk_scope(&request.risk_kinds);
        Self {
            run_id: request.run_id.clone(),
            capability_activation_id: request.capability_activation_id.as_str().to_string(),
            manifest_digest: request.manifest_digest.clone(),
            policy_revision: request.policy_revision,
            origin: request.destination.origin.clone(),
            scheme: request.destination.scheme.clone(),
            ascii_host: request.destination.ascii_host.clone(),
            effective_port: request.destination.effective_port,
            address_class: request.destination.address_class,
            resolution_fingerprint: request.resolution_fingerprint.clone(),
            risk_kinds: request.risk_kinds.clone(),
            exact_target_fingerprint: exact.then(|| request.target_fingerprint.clone()),
            exact_trigger: exact.then_some(request.trigger),
            exact_trigger_tool_name: exact.then(|| request.trigger_tool_name.clone()),
        }
    }
}

fn requires_exact_browser_risk_scope(risk_kinds: &[BrowserRiskKind]) -> bool {
    risk_kinds.iter().any(|risk| {
        matches!(
            risk,
            BrowserRiskKind::UrlUserinfo
                | BrowserRiskKind::RiskEscalation
                | BrowserRiskKind::NewWindow
                | BrowserRiskKind::FileUpload
                | BrowserRiskKind::FileDownload
        )
    })
}

#[derive(Clone, Debug)]
enum BrowserRiskDecision {
    Approved(String),
    Rejected(Option<String>),
    Cancelled,
    PolicyDenied,
}

struct PendingBrowserRisk {
    approval: AgentBrowserRiskApproval,
    conversation_id: String,
    assistant_message_id: String,
    scope: BrowserRiskScope,
    denial_scope: BrowserRiskDenialScope,
    waiters: HashMap<String, oneshot::Sender<BrowserRiskDecision>>,
}

#[derive(Default)]
struct BrowserRiskCoordinatorState {
    by_action_id: HashMap<String, PendingBrowserRisk>,
    action_id_by_scope: HashMap<BrowserRiskScope, String>,
    action_id_by_request_id: HashMap<String, String>,
    cancelled_request_ids: HashSet<String>,
    cancel_tombstone_overflow: bool,
    denied: HashMap<BrowserRiskDenialScope, Option<String>>,
    denial_overflow_runs: HashSet<String>,
    deny_all_overflow: bool,
}

/// Bounded, process-only owner for BrowserRiskApproval waiters and task-level grants.
#[derive(Clone)]
pub(crate) struct BrowserRiskCoordinator {
    runtime: BuiltinCapabilityRuntime,
    state: Arc<Mutex<BrowserRiskCoordinatorState>>,
    clock: BrowserRiskClock,
}

impl std::fmt::Debug for BrowserRiskCoordinator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending_count = self
            .state
            .lock()
            .map(|state| state.by_action_id.len())
            .unwrap_or_default();
        formatter
            .debug_struct("BrowserRiskCoordinator")
            .field("pending_count", &pending_count)
            .finish_non_exhaustive()
    }
}

impl BrowserRiskCoordinator {
    pub(crate) fn new(runtime: BuiltinCapabilityRuntime) -> Arc<Self> {
        Self::with_clock(runtime, Arc::new(unix_timestamp))
    }

    fn with_clock(runtime: BuiltinCapabilityRuntime, clock: BrowserRiskClock) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            state: Arc::new(Mutex::new(BrowserRiskCoordinatorState::default())),
            clock,
        })
    }

    #[cfg(test)]
    pub(crate) async fn authorize(
        &self,
        input: BrowserRiskAuthorizeInput,
        conversation_id: String,
        assistant_message_id: String,
        notifications: CoreServerNotificationSender,
    ) -> BrowserRiskAuthorizeOutput {
        self.authorize_with_permissions(
            input,
            conversation_id,
            assistant_message_id,
            AgentPermissions::default(),
            notifications,
        )
        .await
    }

    /// Resolves a Browser-risk boundary under the exact permissions frozen for the active run.
    /// Automatic approval skips only the UI waiter: request parsing, active capability authority,
    /// destination classification, one-time grant creation, and every Host hard-deny remain on the
    /// same reviewed runtime path as an explicit user approval.
    pub(crate) async fn authorize_with_permissions(
        &self,
        input: BrowserRiskAuthorizeInput,
        conversation_id: String,
        assistant_message_id: String,
        permissions: AgentPermissions,
        notifications: CoreServerNotificationSender,
    ) -> BrowserRiskAuthorizeOutput {
        let request_id = input.request_id.clone();
        let request = match self.parse_request(input) {
            Ok(request) => request,
            Err(_) => {
                return decision(
                    BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                    None,
                    None,
                )
            }
        };
        let rejected_reason = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.deny_all_overflow || state.denial_overflow_runs.contains(&request.run_id) {
                Some(None)
            } else {
                state
                    .denied
                    .get(&BrowserRiskDenialScope::from_request(&request))
                    .cloned()
            }
        };
        if let Some(reason) = rejected_reason {
            return decision(
                BrowserRiskAuthorizationDecisionDto::Rejected,
                None,
                reason.as_deref(),
            );
        }
        match self.runtime.live_browser_risk_grant(&request) {
            Ok(Some(grant)) => {
                return decision(
                    BrowserRiskAuthorizationDecisionDto::Approved,
                    Some(grant.grant_id),
                    None,
                )
            }
            Ok(None) => {}
            Err(_) => {
                return decision(
                    BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                    None,
                    None,
                )
            }
        }

        if permissions.builtin_execution == AgentBuiltinExecutionPermission::AutoApprove {
            let cancelled = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                state.cancel_tombstone_overflow || state.cancelled_request_ids.remove(&request_id)
            };
            if cancelled {
                return decision(BrowserRiskAuthorizationDecisionDto::Cancelled, None, None);
            }
            let mut approval = match self.runtime.prepare_browser_risk_approval(&request) {
                Ok(approval) => approval,
                Err(_) => {
                    return decision(
                        BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                        None,
                        None,
                    )
                }
            };
            approval.approval_status = AgentApprovalStatus::Approved;
            return match self.runtime.approve_browser_risk(&approval) {
                Ok(grant) => decision(
                    BrowserRiskAuthorizationDecisionDto::Approved,
                    Some(grant.grant_id),
                    None,
                ),
                Err(_) => decision(
                    BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                    None,
                    None,
                ),
            };
        }

        let scope = BrowserRiskScope::from_request(&request);
        let (approval, receiver, publish) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.cancel_tombstone_overflow || state.cancelled_request_ids.remove(&request_id) {
                return decision(BrowserRiskAuthorizationDecisionDto::Cancelled, None, None);
            }
            if let Some(action_id) = state.action_id_by_scope.get(&scope).cloned() {
                if let Some(pending) = state.by_action_id.get_mut(&action_id) {
                    let (sender, receiver) = oneshot::channel();
                    pending.waiters.insert(request_id.clone(), sender);
                    let approval = pending.approval.clone();
                    state
                        .action_id_by_request_id
                        .insert(request_id.clone(), action_id);
                    (approval, receiver, false)
                } else {
                    state.action_id_by_scope.remove(&scope);
                    match self.insert_pending(
                        &mut state,
                        request_id.clone(),
                        request,
                        conversation_id,
                        assistant_message_id,
                        scope.clone(),
                    ) {
                        Ok(value) => value,
                        Err(_) => {
                            return decision(
                                BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                                None,
                                None,
                            )
                        }
                    }
                }
            } else {
                match self.insert_pending(
                    &mut state,
                    request_id,
                    request,
                    conversation_id,
                    assistant_message_id,
                    scope.clone(),
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return decision(
                            BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                            None,
                            None,
                        )
                    }
                }
            }
        };

        if publish
            && notifications
                .send(approval_notification(&approval))
                .is_err()
        {
            self.settle_without_grant(
                &approval.run_id,
                &approval.action_id,
                BrowserRiskDecision::Cancelled,
            );
            return decision(BrowserRiskAuthorizationDecisionDto::Cancelled, None, None);
        }

        match receiver.await {
            Ok(BrowserRiskDecision::Approved(grant_id)) => decision(
                BrowserRiskAuthorizationDecisionDto::Approved,
                Some(grant_id),
                None,
            ),
            Ok(BrowserRiskDecision::Rejected(reason)) => decision(
                BrowserRiskAuthorizationDecisionDto::Rejected,
                None,
                reason.as_deref(),
            ),
            Ok(BrowserRiskDecision::Cancelled) => {
                decision(BrowserRiskAuthorizationDecisionDto::Cancelled, None, None)
            }
            Ok(BrowserRiskDecision::PolicyDenied) => decision(
                BrowserRiskAuthorizationDecisionDto::PolicyDenied,
                None,
                None,
            ),
            Err(_) => decision(BrowserRiskAuthorizationDecisionDto::Cancelled, None, None),
        }
    }

    fn insert_pending(
        &self,
        state: &mut BrowserRiskCoordinatorState,
        request_id: String,
        request: BrowserRiskAuthorizationRequest,
        conversation_id: String,
        assistant_message_id: String,
        scope: BrowserRiskScope,
    ) -> Result<
        (
            AgentBrowserRiskApproval,
            oneshot::Receiver<BrowserRiskDecision>,
            bool,
        ),
        (),
    > {
        if state.by_action_id.len() >= MAX_PENDING_BROWSER_RISK_APPROVALS {
            return Err(());
        }
        let approval = self
            .runtime
            .prepare_browser_risk_approval(&request)
            .map_err(|_| ())?;
        let (sender, receiver) = oneshot::channel();
        state
            .action_id_by_scope
            .insert(scope.clone(), approval.action_id.clone());
        state
            .action_id_by_request_id
            .insert(request_id.clone(), approval.action_id.clone());
        state.by_action_id.insert(
            approval.action_id.clone(),
            PendingBrowserRisk {
                approval: approval.clone(),
                conversation_id,
                assistant_message_id,
                scope,
                denial_scope: BrowserRiskDenialScope::from_request(&request),
                waiters: HashMap::from([(request_id, sender)]),
            },
        );
        Ok((approval, receiver, true))
    }

    pub(crate) fn pending_conversation_id(&self, run_id: &str, action_id: &str) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .by_action_id
            .get(action_id)
            .filter(|pending| pending.approval.run_id == run_id)
            .map(|pending| pending.conversation_id.clone())
    }

    pub(crate) fn list_pending(&self) -> Vec<PendingAgentActionSnapshot> {
        let mut pending = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .by_action_id
            .values()
            .map(|pending| {
                pending_snapshot(
                    &pending.approval,
                    &pending.conversation_id,
                    &pending.assistant_message_id,
                )
            })
            .collect::<Vec<_>>();
        pending.sort_by_key(|snapshot| snapshot.created_at);
        pending
    }

    pub(crate) fn approve(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let Some(mut pending) = self.take_pending(run_id, action_id) else {
            return Ok(None);
        };
        pending.approval.approval_status = AgentApprovalStatus::Approved;
        match self.runtime.approve_browser_risk(&pending.approval) {
            Ok(grant) => {
                notify_waiters(
                    pending.waiters,
                    BrowserRiskDecision::Approved(grant.grant_id),
                );
                Ok(Some(execution_output(&pending.approval, "approved")))
            }
            Err(_) => {
                notify_waiters(pending.waiters, BrowserRiskDecision::PolicyDenied);
                // The human decision is authoritative. Runtime authority (including the
                // short-lived destination binding) may have expired while the ticket waited;
                // report that as the original Browser Tool's ordinary failure instead of
                // rejecting the approval RPC and stranding its card.
                Ok(Some(execution_output(&pending.approval, "failed")))
            }
        }
    }

    pub(crate) fn reject(
        &self,
        run_id: &str,
        action_id: &str,
        reason: Option<String>,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let reason = bounded_reason(reason)?;
        let Some(pending) = self.take_pending(run_id, action_id) else {
            return Ok(None);
        };
        // Rejection is authoritative even after the process-only runtime binding expires.
        let _ = self
            .runtime
            .dismiss_browser_risk_approval(&pending.approval);
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.denied.len() >= MAX_PENDING_BROWSER_RISK_APPROVALS
                && !state.denied.contains_key(&pending.denial_scope)
            {
                if state.denial_overflow_runs.len() < MAX_PENDING_BROWSER_RISK_APPROVALS {
                    state
                        .denial_overflow_runs
                        .insert(pending.approval.run_id.clone());
                } else {
                    // Never evict a refusal: that could make an unchanged denied action prompt
                    // again. Saturation instead fails closed with bounded process memory.
                    state.deny_all_overflow = true;
                }
            } else {
                state
                    .denied
                    .insert(pending.denial_scope.clone(), reason.clone());
            }
        }
        notify_waiters(
            pending.waiters,
            BrowserRiskDecision::Rejected(reason.clone()),
        );
        Ok(Some(execution_output(&pending.approval, "rejected")))
    }

    pub(crate) fn cancel(&self, run_id: &str, action_id: &str) -> Result<Option<bool>, String> {
        let Some(pending) = self.take_pending(run_id, action_id) else {
            return Ok(None);
        };
        // Cancellation is authoritative even after the process-only runtime binding expires.
        let _ = self
            .runtime
            .dismiss_browser_risk_approval(&pending.approval);
        notify_waiters(pending.waiters, BrowserRiskDecision::Cancelled);
        Ok(Some(true))
    }

    pub(crate) fn cancel_run(&self, run_id: &str) {
        let actions = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .by_action_id
            .iter()
            .filter_map(|(action_id, pending)| {
                (pending.approval.run_id == run_id).then_some(action_id.clone())
            })
            .collect::<Vec<_>>();
        for action_id in actions {
            self.settle_without_grant(run_id, &action_id, BrowserRiskDecision::Cancelled);
        }
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state
                .denied
                .retain(|scope, _| scope.grant_scope.run_id != run_id);
            state.denial_overflow_runs.remove(run_id);
        }
        // Capability and risk grants are task authority, not a cache. A terminal/cancelled run
        // retires them immediately; the Host stops managed automation only when no other run owns
        // a live grant, while keeping the visible Browser surface intact.
        let _ = self.runtime.revoke_run_grants(run_id);
    }

    pub(crate) fn cancel_all(&self) {
        let actions = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .by_action_id
            .iter()
            .map(|(action_id, pending)| (pending.approval.run_id.clone(), action_id.clone()))
            .collect::<Vec<_>>();
        for (run_id, action_id) in actions {
            self.settle_without_grant(&run_id, &action_id, BrowserRiskDecision::Cancelled);
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.denied.clear();
        state.denial_overflow_runs.clear();
        state.deny_all_overflow = false;
        state.cancelled_request_ids.clear();
        state.cancel_tombstone_overflow = false;
    }

    pub(crate) fn cancel_request(&self, input: BrowserRiskCancelInput) -> BrowserRiskCancelOutput {
        let accepted = if input.schema_version != BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
            || !valid_v4_uuid(&input.request_id)
        {
            false
        } else {
            self.cancel_request_id(&input.request_id)
        };
        BrowserRiskCancelOutput {
            schema_version: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
            accepted,
        }
    }

    fn cancel_request_id(&self, request_id: &str) -> bool {
        let (waiter, removed_pending) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let Some(action_id) = state.action_id_by_request_id.remove(request_id) else {
                if state.cancelled_request_ids.len() >= MAX_PENDING_BROWSER_RISK_APPROVALS {
                    state.cancel_tombstone_overflow = true;
                } else {
                    state.cancelled_request_ids.insert(request_id.to_string());
                }
                return true;
            };
            let Some(pending) = state.by_action_id.get_mut(&action_id) else {
                return false;
            };
            let waiter = pending.waiters.remove(request_id);
            let empty = pending.waiters.is_empty();
            if empty {
                let pending = state.by_action_id.remove(&action_id);
                if let Some(pending) = pending.as_ref() {
                    state.action_id_by_scope.remove(&pending.scope);
                }
                (waiter, pending)
            } else {
                (waiter, None)
            }
        };
        if let Some(waiter) = waiter {
            let _ = waiter.send(BrowserRiskDecision::Cancelled);
        }
        if let Some(pending) = removed_pending {
            let _ = self
                .runtime
                .dismiss_browser_risk_approval(&pending.approval);
        }
        true
    }

    fn settle_without_grant(&self, run_id: &str, action_id: &str, decision: BrowserRiskDecision) {
        let Some(pending) = self.take_pending(run_id, action_id) else {
            return;
        };
        let _ = self
            .runtime
            .dismiss_browser_risk_approval(&pending.approval);
        notify_waiters(pending.waiters, decision);
    }

    fn take_pending(&self, run_id: &str, action_id: &str) -> Option<PendingBrowserRisk> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state
            .by_action_id
            .get(action_id)
            .is_none_or(|pending| pending.approval.run_id != run_id)
        {
            return None;
        }
        let pending = state.by_action_id.remove(action_id)?;
        state.action_id_by_scope.remove(&pending.scope);
        for request_id in pending.waiters.keys() {
            state.action_id_by_request_id.remove(request_id);
        }
        Some(pending)
    }

    fn parse_request(
        &self,
        input: BrowserRiskAuthorizeInput,
    ) -> Result<BrowserRiskAuthorizationRequest, ()> {
        if input.schema_version != BROWSER_RISK_PROTOCOL_SCHEMA_VERSION
            || !valid_v4_uuid(&input.request_id)
            || input
                .parent_request_id
                .as_deref()
                .is_some_and(|value| !valid_v4_uuid(value))
            || input.authorization_context.run_id.trim().is_empty()
            || input.authorization_context.run_id.len() > MAX_CALL_ID_BYTES
            || input.authorization_context.call_id.trim().is_empty()
            || input.authorization_context.call_id.len() > MAX_CALL_ID_BYTES
            || input.authorization_context.call_reason.trim().is_empty()
            || input.authorization_context.call_reason.len() > MAX_SAFE_REASON_BYTES
            || !valid_v4_uuid(&input.authorization_context.invocation_id)
            || !input
                .authorization_context
                .grant_expires_at_ms
                .is_multiple_of(1_000)
            || input.created_at_ms >= input.expires_at_ms
            || input.expires_at_ms.saturating_sub(input.created_at_ms)
                > BROWSER_RISK_APPROVAL_TTL_SECONDS.saturating_mul(1_000)
            || input.expires_at_ms <= (self.clock)().saturating_mul(1_000)
        {
            return Err(());
        }
        let capability_id = BuiltinCapabilityId::parse(input.authorization_context.capability_id)
            .map_err(|_| ())?;
        let capability_activation_id =
            CapabilityActivationId::parse(input.authorization_context.activation_id)
                .map_err(|_| ())?;
        let display_name = self
            .runtime
            .manifests()
            .iter()
            .find(|manifest| manifest.descriptor.id == capability_id)
            .map(|manifest| manifest.descriptor.display_name.clone())
            .ok_or(())?;
        let mut risk_kinds = input
            .risk_kinds
            .into_iter()
            .map(map_risk_kind)
            .collect::<Vec<_>>();
        risk_kinds.sort_unstable();
        if risk_kinds.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(());
        }
        let request = BrowserRiskAuthorizationRequest {
            run_id: input.authorization_context.run_id,
            call_id: input.authorization_context.call_id,
            trigger_tool_name: input.authorization_context.trigger_tool_name,
            capability_id,
            capability_activation_id,
            manifest_digest: input.authorization_context.manifest_digest,
            policy_revision: input.authorization_context.policy_revision,
            display_name,
            reason: input.authorization_context.call_reason,
            destination: BrowserDestinationIdentity {
                normalized_url: input.destination.normalized_url,
                origin: input.destination.origin,
                scheme: input.destination.scheme,
                ascii_host: input.destination.ascii_host,
                effective_port: input.destination.effective_port,
                address_class: map_address_class(input.destination.address_class),
            },
            resolution_fingerprint: input.destination.resolution_fingerprint,
            target_fingerprint: input.destination.target_fingerprint,
            trigger: map_trigger(input.trigger),
            risk_kinds,
        };
        request.validate().map_err(|_| ())?;
        Ok(request)
    }
}

fn approval_notification(approval: &AgentBrowserRiskApproval) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": mycopilot_protocol_rs::AGENT_EVENT_NOTIFICATION_METHOD,
        "params": {
            "type": "approval_required",
            "runId": approval.run_id,
            "action": AgentProposedAction::BrowserRiskApproval {
                approval: Box::new(approval.clone()),
            },
        },
    })
}

fn pending_snapshot(
    approval: &AgentBrowserRiskApproval,
    conversation_id: &str,
    assistant_message_id: &str,
) -> PendingAgentActionSnapshot {
    PendingAgentActionSnapshot {
        action_id: approval.action_id.clone(),
        action_type: "browser_risk_approval".to_string(),
        tool_name: approval.trigger_tool_name.clone(),
        tool_call_id: Some(approval.call_id.clone()),
        run_id: approval.run_id.clone(),
        conversation_id: Some(conversation_id.to_string()),
        assistant_message_id: Some(assistant_message_id.to_string()),
        action: AgentProposedAction::BrowserRiskApproval {
            approval: Box::new(approval.clone()),
        },
        created_at: i64::try_from(approval.created_at).unwrap_or(i64::MAX),
        status: PendingActionStatus::Pending,
    }
}

fn execution_output(
    approval: &AgentBrowserRiskApproval,
    status: &str,
) -> AgentActionExecutionOutput {
    AgentActionExecutionOutput {
        action_id: approval.action_id.clone(),
        action_type: "browser_risk_approval".to_string(),
        tool_name: approval.trigger_tool_name.clone(),
        status: status.to_string(),
        file_change_result: None,
        command_result: None,
        tool_result: None,
        agent_output: AgentChatOutput {
            content: String::new(),
            status: AgentRunStatus::Running,
            run_id: approval.run_id.clone(),
            events: Vec::new(),
            tool_definitions: Vec::new(),
            todo: None,
            usage: None,
            finish_reason: None,
            proposed_actions: Vec::new(),
            conversation_turn_trace: None,
        },
    }
}

fn notify_waiters(
    waiters: HashMap<String, oneshot::Sender<BrowserRiskDecision>>,
    decision: BrowserRiskDecision,
) {
    for waiter in waiters.into_values() {
        let _ = waiter.send(decision.clone());
    }
}

fn decision(
    decision: BrowserRiskAuthorizationDecisionDto,
    grant_id: Option<String>,
    reason: Option<&str>,
) -> BrowserRiskAuthorizeOutput {
    BrowserRiskAuthorizeOutput {
        schema_version: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
        decision,
        grant_id,
        reason: reason.map(|reason| reason.chars().take(MAX_SAFE_REASON_BYTES).collect()),
    }
}

fn bounded_reason(reason: Option<String>) -> Result<Option<String>, String> {
    match reason {
        Some(reason)
            if reason.len() > MAX_SAFE_REASON_BYTES || reason.chars().any(char::is_control) =>
        {
            Err("Browser risk rejection reason is invalid.".to_string())
        }
        Some(reason) if reason.trim().is_empty() => Ok(None),
        value => Ok(value),
    }
}

fn valid_v4_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uuid| {
        !uuid.is_nil() && uuid.get_version() == Some(Version::Random) && uuid.to_string() == value
    })
}

fn map_address_class(value: BrowserResolvedAddressClassDto) -> BrowserResolvedAddressClass {
    match value {
        BrowserResolvedAddressClassDto::Public => BrowserResolvedAddressClass::Public,
        BrowserResolvedAddressClassDto::Loopback => BrowserResolvedAddressClass::Loopback,
        BrowserResolvedAddressClassDto::Private => BrowserResolvedAddressClass::Private,
        BrowserResolvedAddressClassDto::LinkLocal => BrowserResolvedAddressClass::LinkLocal,
        BrowserResolvedAddressClassDto::CloudMetadata => BrowserResolvedAddressClass::CloudMetadata,
        BrowserResolvedAddressClassDto::Unresolved => BrowserResolvedAddressClass::Unresolved,
    }
}

fn map_trigger(value: BrowserRiskTriggerDto) -> BrowserRiskTrigger {
    match value {
        BrowserRiskTriggerDto::ToolArgument => BrowserRiskTrigger::ToolArgument,
        BrowserRiskTriggerDto::MainFrame => BrowserRiskTrigger::MainFrame,
        BrowserRiskTriggerDto::Redirect => BrowserRiskTrigger::Redirect,
        BrowserRiskTriggerDto::NewWindow => BrowserRiskTrigger::NewWindow,
        BrowserRiskTriggerDto::Subresource => BrowserRiskTrigger::Subresource,
        BrowserRiskTriggerDto::Upload => BrowserRiskTrigger::Upload,
        BrowserRiskTriggerDto::Download => BrowserRiskTrigger::Download,
    }
}

fn map_risk_kind(value: BrowserRiskKindDto) -> BrowserRiskKind {
    match value {
        BrowserRiskKindDto::InsecureHttp => BrowserRiskKind::InsecureHttp,
        BrowserRiskKindDto::Localhost => BrowserRiskKind::Localhost,
        BrowserRiskKindDto::Loopback => BrowserRiskKind::Loopback,
        BrowserRiskKindDto::PrivateNetwork => BrowserRiskKind::PrivateNetwork,
        BrowserRiskKindDto::LinkLocal => BrowserRiskKind::LinkLocal,
        BrowserRiskKindDto::CloudMetadata => BrowserRiskKind::CloudMetadata,
        BrowserRiskKindDto::NonStandardPort => BrowserRiskKind::NonStandardPort,
        BrowserRiskKindDto::UrlUserinfo => BrowserRiskKind::UrlUserinfo,
        BrowserRiskKindDto::DnsPrivateResolution => BrowserRiskKind::DnsPrivateResolution,
        BrowserRiskKindDto::RiskEscalation => BrowserRiskKind::RiskEscalation,
        BrowserRiskKindDto::NewWindow => BrowserRiskKind::NewWindow,
        BrowserRiskKindDto::FileUpload => BrowserRiskKind::FileUpload,
        BrowserRiskKindDto::FileDownload => BrowserRiskKind::FileDownload,
        BrowserRiskKindDto::LocalServiceRequest => BrowserRiskKind::LocalServiceRequest,
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Drop for BrowserRiskCoordinator {
    fn drop(&mut self) {
        // Only the final Arc owner performs cleanup. A process restart therefore turns every
        // process-only pending approval into cancellation rather than replaying it.
        if Arc::strong_count(&self.state) == 1 {
            self.cancel_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::mcp::builtin_capability_policy::{
        BuiltinCapabilityId as StoredCapabilityId, SqliteBuiltinCapabilityPolicyStore,
    };
    use crate::application::mcp::builtin_capability_runtime::HostBuiltinCapabilityProvider;
    use crate::application::mcp::playwright_manifest::BROWSER_AUTOMATION_CAPABILITY_ID;
    use mycopilot_core::{
        AgentBuiltinCapabilityActivationApproval, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    };
    use mycopilot_protocol_rs::{
        BrowserRiskDestinationInput, BrowserRiskDispatchCertaintyDto,
        ManagedPlaywrightAuthorizationContext,
    };

    struct Harness {
        _directory: tempfile::TempDir,
        _storage: mycopilot_core::storage::service::StorageService,
        coordinator: Arc<BrowserRiskCoordinator>,
        grant: mycopilot_core::CapabilityGrant,
    }

    fn harness() -> Harness {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("storage.sqlite");
        let storage = mycopilot_core::storage::service::StorageService::open(&path).unwrap();
        let policies = Arc::new(SqliteBuiltinCapabilityPolicyStore::open(&path).unwrap());
        policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let (runtime, _) =
            HostBuiltinCapabilityProvider::runtime_and_provider(Arc::clone(&policies), None)
                .unwrap();
        let manifest = runtime.manifests()[0].clone();
        let now = unix_timestamp();
        let approval = AgentBuiltinCapabilityActivationApproval {
            action_id: Uuid::new_v4().to_string(),
            activation_id: Uuid::new_v4().to_string(),
            run_id: "browser-risk-run".to_string(),
            call_id: "activation-call".to_string(),
            capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
            display_name: manifest.descriptor.display_name,
            reason: "Use browser automation.".to_string(),
            manifest_digest: manifest.manifest_digest,
            policy_revision: 1,
            created_at: now,
            expires_at: now + BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
            approval_status: AgentApprovalStatus::Approved,
        };
        let grant = runtime.approve_activation(&approval).unwrap();
        Harness {
            _directory: directory,
            _storage: storage,
            coordinator: BrowserRiskCoordinator::new(runtime),
            grant,
        }
    }

    fn input(harness: &Harness, request_id: Uuid) -> BrowserRiskAuthorizeInput {
        let now_ms = unix_timestamp() * 1_000;
        BrowserRiskAuthorizeInput {
            schema_version: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
            request_id: request_id.to_string(),
            parent_request_id: None,
            authorization_context: ManagedPlaywrightAuthorizationContext {
                conversation_id: None,
                run_id: harness.grant.run_id.clone(),
                capability_id: harness.grant.capability_id.as_str().to_string(),
                activation_id: harness.grant.activation_id.as_str().to_string(),
                manifest_digest: harness.grant.manifest_digest.clone(),
                policy_revision: harness.grant.policy_revision,
                grant_expires_at_ms: harness.grant.expires_at * 1_000,
                invocation_id: Uuid::new_v4().to_string(),
                call_id: "browser-call".to_string(),
                trigger_tool_name: "browser_navigate".to_string(),
                call_reason: "Open the private fixture.".to_string(),
                builtin_tool_grant: None,
            },
            destination: BrowserRiskDestinationInput {
                normalized_url: "http://10.0.0.1:8080/a".to_string(),
                origin: "http://10.0.0.1:8080".to_string(),
                scheme: "http".to_string(),
                ascii_host: "10.0.0.1".to_string(),
                effective_port: 8080,
                address_class: BrowserResolvedAddressClassDto::Private,
                resolution_fingerprint: format!("hmac-sha256:{}", "b".repeat(64)),
                target_fingerprint: format!("hmac-sha256:{}", "c".repeat(64)),
            },
            // Main emits lexical order. The Host canonicalizes this into the frozen Rust order
            // after rejecting duplicates, so cross-language order never becomes authority.
            risk_kinds: vec![
                BrowserRiskKindDto::DnsPrivateResolution,
                BrowserRiskKindDto::LocalServiceRequest,
                BrowserRiskKindDto::PrivateNetwork,
            ],
            trigger: BrowserRiskTriggerDto::Redirect,
            dispatch_certainty: BrowserRiskDispatchCertaintyDto::PossiblyDispatched,
            created_at_ms: now_ms,
            expires_at_ms: now_ms + BROWSER_RISK_APPROVAL_TTL_SECONDS * 1_000,
        }
    }

    async fn wait_for_one_pending(coordinator: &BrowserRiskCoordinator) -> String {
        for _ in 0..100 {
            let pending = coordinator.list_pending();
            if let Some(snapshot) = pending.first() {
                assert_eq!(pending.len(), 1);
                assert_eq!(snapshot.assistant_message_id.as_deref(), Some("assistant"));
                return snapshot.action_id.clone();
            }
            tokio::task::yield_now().await;
        }
        panic!("browser risk approval was not published");
    }

    #[tokio::test]
    async fn post_dispatch_boundary_still_prompts_and_concurrent_requests_deduplicate() {
        let harness = harness();
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        let first = {
            let coordinator = Arc::clone(&harness.coordinator);
            let notifications = notifications.clone();
            let input = input(&harness, Uuid::new_v4());
            tokio::spawn(async move {
                coordinator
                    .authorize(
                        input,
                        "conversation".to_string(),
                        "assistant".to_string(),
                        notifications,
                    )
                    .await
            })
        };
        let second = {
            let coordinator = Arc::clone(&harness.coordinator);
            let notifications = notifications.clone();
            let input = input(&harness, Uuid::new_v4());
            tokio::spawn(async move {
                coordinator
                    .authorize(
                        input,
                        "conversation".to_string(),
                        "assistant".to_string(),
                        notifications,
                    )
                    .await
            })
        };
        let action_id = wait_for_one_pending(&harness.coordinator).await;
        assert!(harness
            .coordinator
            .approve(&harness.grant.run_id, &action_id)
            .unwrap()
            .is_some());
        for output in [first.await.unwrap(), second.await.unwrap()] {
            assert_eq!(
                output.decision,
                BrowserRiskAuthorizationDecisionDto::Approved
            );
            assert!(output.grant_id.is_some());
        }
        assert!(harness.coordinator.list_pending().is_empty());
    }

    #[tokio::test]
    async fn late_approval_is_accepted_and_reports_expired_runtime_authority_to_the_waiter() {
        let harness = harness();
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        let authorization = {
            let coordinator = Arc::clone(&harness.coordinator);
            let request = input(&harness, Uuid::new_v4());
            tokio::spawn(async move {
                coordinator
                    .authorize(
                        request,
                        "conversation".to_string(),
                        "assistant".to_string(),
                        notifications,
                    )
                    .await
            })
        };
        let action_id = wait_for_one_pending(&harness.coordinator).await;
        {
            let mut state = harness.coordinator.state.lock().unwrap();
            state
                .by_action_id
                .get_mut(&action_id)
                .expect("approval remains pending for the human decision")
                .approval
                .expires_at = unix_timestamp().saturating_sub(1);
        }

        let approval = harness
            .coordinator
            .approve(&harness.grant.run_id, &action_id)
            .unwrap()
            .expect("the late decision is accepted");
        assert_eq!(approval.status, "failed");
        assert_eq!(
            authorization.await.unwrap().decision,
            BrowserRiskAuthorizationDecisionDto::PolicyDenied
        );
        assert!(harness.coordinator.list_pending().is_empty());
    }

    #[tokio::test]
    async fn trusted_builtin_auto_permission_mints_the_same_risk_grant_without_a_ui_waiter() {
        let harness = harness();
        let (notifications, mut events) = tokio::sync::mpsc::unbounded_channel();
        let permissions = AgentPermissions {
            builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
            ..AgentPermissions::default()
        };

        let output = harness
            .coordinator
            .authorize_with_permissions(
                input(&harness, Uuid::new_v4()),
                "conversation".to_string(),
                "assistant".to_string(),
                permissions,
                notifications,
            )
            .await;

        assert_eq!(
            output.decision,
            BrowserRiskAuthorizationDecisionDto::Approved
        );
        assert!(output.grant_id.is_some());
        assert!(harness.coordinator.list_pending().is_empty());
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn explicit_rejection_is_a_normal_result_and_exact_retry_does_not_prompt_again() {
        let harness = harness();
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        let original = input(&harness, Uuid::new_v4());
        let pending = {
            let coordinator = Arc::clone(&harness.coordinator);
            let notifications = notifications.clone();
            tokio::spawn(async move {
                coordinator
                    .authorize(
                        original,
                        "conversation".to_string(),
                        "assistant".to_string(),
                        notifications,
                    )
                    .await
            })
        };
        let action_id = wait_for_one_pending(&harness.coordinator).await;
        harness
            .coordinator
            .reject(
                &harness.grant.run_id,
                &action_id,
                Some("Do not access this service.".to_string()),
            )
            .unwrap();
        let rejected = pending.await.unwrap();
        assert_eq!(
            rejected.decision,
            BrowserRiskAuthorizationDecisionDto::Rejected
        );
        assert_eq!(
            rejected.reason.as_deref(),
            Some("Do not access this service.")
        );

        let repeated = harness
            .coordinator
            .authorize(
                input(&harness, Uuid::new_v4()),
                "conversation".to_string(),
                "assistant".to_string(),
                notifications,
            )
            .await;
        assert_eq!(
            repeated.decision,
            BrowserRiskAuthorizationDecisionDto::Rejected
        );
        assert!(harness.coordinator.list_pending().is_empty());
    }

    #[tokio::test]
    async fn concurrent_approve_reject_has_exactly_one_winner() {
        let harness = harness();
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        let authorization = {
            let coordinator = Arc::clone(&harness.coordinator);
            let request = input(&harness, Uuid::new_v4());
            tokio::spawn(async move {
                coordinator
                    .authorize(
                        request,
                        "conversation".to_string(),
                        "assistant".to_string(),
                        notifications,
                    )
                    .await
            })
        };
        let action_id = wait_for_one_pending(&harness.coordinator).await;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let approve = {
            let coordinator = Arc::clone(&harness.coordinator);
            let barrier = Arc::clone(&barrier);
            let run_id = harness.grant.run_id.clone();
            let action_id = action_id.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                coordinator.approve(&run_id, &action_id)
            })
        };
        let reject = {
            let coordinator = Arc::clone(&harness.coordinator);
            let barrier = Arc::clone(&barrier);
            let run_id = harness.grant.run_id.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                coordinator.reject(&run_id, &action_id, Some("No.".to_string()))
            })
        };
        let approve_won = approve.await.unwrap().unwrap().is_some();
        let reject_won = reject.await.unwrap().unwrap().is_some();
        assert_ne!(approve_won, reject_won);
        assert!(matches!(
            authorization.await.unwrap().decision,
            BrowserRiskAuthorizationDecisionDto::Approved
                | BrowserRiskAuthorizationDecisionDto::Rejected
        ));
        assert!(harness.coordinator.list_pending().is_empty());
    }

    #[tokio::test]
    async fn cancellation_before_authorize_registration_fails_closed_without_ghost_prompt() {
        let harness = harness();
        let request_id = Uuid::new_v4();
        assert!(
            harness
                .coordinator
                .cancel_request(BrowserRiskCancelInput {
                    schema_version: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
                    request_id: request_id.to_string(),
                })
                .accepted
        );
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        let output = harness
            .coordinator
            .authorize(
                input(&harness, request_id),
                "conversation".to_string(),
                "assistant".to_string(),
                notifications,
            )
            .await;
        assert_eq!(
            output.decision,
            BrowserRiskAuthorizationDecisionDto::Cancelled
        );
        assert!(harness.coordinator.list_pending().is_empty());
    }

    #[tokio::test]
    async fn refusal_and_cancel_tombstone_storms_remain_bounded_and_fail_closed() {
        let denial_harness = harness();
        let (notifications, _events) = tokio::sync::mpsc::unbounded_channel();
        for index in 0..=MAX_PENDING_BROWSER_RISK_APPROVALS {
            let mut request = input(&denial_harness, Uuid::new_v4());
            request.destination.normalized_url = format!("http://10.0.0.1:8080/denied-{index}");
            request.destination.target_fingerprint = format!("hmac-sha256:{index:064x}");
            let pending = {
                let coordinator = Arc::clone(&denial_harness.coordinator);
                let notifications = notifications.clone();
                tokio::spawn(async move {
                    coordinator
                        .authorize(
                            request,
                            "conversation".to_string(),
                            "assistant".to_string(),
                            notifications,
                        )
                        .await
                })
            };
            let action_id = wait_for_one_pending(&denial_harness.coordinator).await;
            denial_harness
                .coordinator
                .reject(&denial_harness.grant.run_id, &action_id, None)
                .unwrap();
            assert_eq!(
                pending.await.unwrap().decision,
                BrowserRiskAuthorizationDecisionDto::Rejected
            );
        }
        {
            let state = denial_harness.coordinator.state.lock().unwrap();
            assert_eq!(state.denied.len(), MAX_PENDING_BROWSER_RISK_APPROVALS);
            assert!(state
                .denial_overflow_runs
                .contains(&denial_harness.grant.run_id));
        }
        let denied_without_prompt = denial_harness
            .coordinator
            .authorize(
                input(&denial_harness, Uuid::new_v4()),
                "conversation".to_string(),
                "assistant".to_string(),
                notifications.clone(),
            )
            .await;
        assert_eq!(
            denied_without_prompt.decision,
            BrowserRiskAuthorizationDecisionDto::Rejected
        );
        assert!(denial_harness.coordinator.list_pending().is_empty());

        let cancellation_harness = harness();
        for _ in 0..=MAX_PENDING_BROWSER_RISK_APPROVALS {
            assert!(
                cancellation_harness
                    .coordinator
                    .cancel_request(BrowserRiskCancelInput {
                        schema_version: BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
                        request_id: Uuid::new_v4().to_string(),
                    })
                    .accepted
            );
        }
        {
            let state = cancellation_harness.coordinator.state.lock().unwrap();
            assert_eq!(
                state.cancelled_request_ids.len(),
                MAX_PENDING_BROWSER_RISK_APPROVALS
            );
            assert!(state.cancel_tombstone_overflow);
        }
        let cancelled_without_prompt = cancellation_harness
            .coordinator
            .authorize(
                input(&cancellation_harness, Uuid::new_v4()),
                "conversation".to_string(),
                "assistant".to_string(),
                notifications,
            )
            .await;
        assert_eq!(
            cancelled_without_prompt.decision,
            BrowserRiskAuthorizationDecisionDto::Cancelled
        );
        assert!(cancellation_harness.coordinator.list_pending().is_empty());
    }
}
