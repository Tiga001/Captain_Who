//! Catalog-bound tool dispatch, admission control, and active-call settlement.

use super::*;

impl McpConnectionManager {
    pub fn active_call(
        &self,
        id: &McpActiveCallId,
    ) -> Result<Option<McpActiveCallSnapshot>, McpError> {
        let Some(entry) = self.get_entry(id.server_id)? else {
            return Ok(None);
        };
        let snapshot = lock_entry(&entry)?
            .active_calls
            .get(id)
            .map(|call| call.snapshot());
        Ok(snapshot)
    }

    pub fn list_active_calls(
        &self,
        server_id: Option<McpServerId>,
    ) -> Result<Vec<McpActiveCallSnapshot>, McpError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        let mut calls = Vec::new();
        for (entry_server_id, entry) in entries.iter() {
            if server_id.is_some_and(|expected| expected != *entry_server_id) {
                continue;
            }
            calls.extend(
                lock_entry(entry)?
                    .active_calls
                    .values()
                    .map(|call| call.snapshot()),
            );
        }
        calls.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(calls)
    }

    pub fn active_call_count(&self) -> Result<usize, McpError> {
        Ok(self.list_active_calls(None)?.len())
    }

    pub fn resolve_model_name(&self, model_name: &str) -> Result<Option<McpToolId>, McpError> {
        let entries = self
            .inner
            .entries
            .lock()
            .map_err(|_| McpError::protocol("MCP manager entries lock is unavailable"))?;
        let mut resolved = None;
        for entry in entries.values() {
            let state = lock_entry(entry)?;
            if state.removed
                || !state.status.enabled
                || state.status.trust == McpTrustLevel::Untrusted
                || !matches!(
                    state.status.state,
                    McpServerState::Ready | McpServerState::Degraded
                )
                || state.peer.is_none()
                || state.catalog.source_config_epoch != Some(state.status.config_epoch)
                || state.catalog.source_registry_revision != Some(state.status.registry_revision)
                || state.catalog.source_config_digest.as_ref() != Some(&state.status.config_digest)
                || state.catalog.completeness != McpCatalogCompleteness::Complete
            {
                continue;
            }
            if let Some(tool_id) = state.catalog.resolve_model_name(model_name) {
                if resolved.is_some() {
                    return Err(McpError::protocol(
                        "MCP model tool name is ambiguous in the catalog",
                    ));
                }
                resolved = Some(tool_id.clone());
            }
        }
        Ok(resolved)
    }

    /// Invoke a tool only if the caller's catalog and configuration snapshot
    /// still identify the active, complete catalog.
    ///
    /// All routing checks happen while briefly holding the per-server state
    /// lock. The peer and raw call are cloned out before the protocol request
    /// is awaited.
    pub async fn call_catalog_tool(
        &self,
        request: McpCatalogToolCall,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        let invocation_id = McpInvocationId::new();
        let model_call_id = McpModelCallId::new(format!("legacy-{invocation_id}"))?;
        let id = McpActiveCallId::new(request.tool_id.server_id, invocation_id, model_call_id);
        self.call_catalog_tool_identified(id, request, cancellation)
            .await
    }

    /// Invoke a catalog-bound tool using the Host/Runtime identity that will
    /// also appear in approval records, traces and checkpoints.
    pub async fn call_catalog_tool_identified(
        &self,
        id: McpActiveCallId,
        request: McpCatalogToolCall,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        let server_id = request.tool_id.server_id;
        if id.server_id != server_id {
            return Err(McpError::config(
                "MCP active-call identity does not match the catalog route",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(McpError::cancelled("MCP tools/call"));
        }
        let registry =
            self.inner.registry.get(server_id)?.ok_or_else(|| {
                McpError::config("MCP catalog invocation server is not registered")
            })?;
        if !registry.config.enabled {
            return Err(McpError::config(
                "MCP catalog invocation server is disabled",
            ));
        }
        if registry.config.trust == McpTrustLevel::Untrusted {
            return Err(McpError::config(
                "MCP catalog invocation server is not trusted",
            ));
        }
        if registry.config.approval_mode == McpApprovalMode::Deny {
            return Err(McpError::config(
                "MCP tool invocation is denied by server approval policy",
            ));
        }
        validate_tool_arguments(&request.arguments, &self.inner.policy.security_limits)?;
        let entry = self
            .get_entry(server_id)?
            .ok_or_else(|| McpError::config("MCP catalog invocation server is not ready"))?;

        let (peer, call, control, active_status, global_permit) = {
            let mut state = lock_entry(&entry)?;
            if state.removed {
                return Err(McpError::config(
                    "MCP catalog invocation server is not registered",
                ));
            }
            if !state.status.enabled {
                return Err(McpError::config(
                    "MCP catalog invocation server is disabled",
                ));
            }
            if state.status.trust == McpTrustLevel::Untrusted {
                return Err(McpError::config(
                    "MCP catalog invocation server is not trusted",
                ));
            }
            if state.status.approval_mode == McpApprovalMode::Deny {
                return Err(McpError::config(
                    "MCP tool invocation is denied by server approval policy",
                ));
            }
            if state.status.state != McpServerState::Ready {
                return Err(McpError::config(
                    "MCP catalog invocation server is not ready",
                ));
            }
            let peer = state.peer.clone().ok_or_else(|| {
                McpError::protocol("MCP catalog invocation active peer is unavailable")
            })?;
            if peer.server_id() != server_id || state.catalog.server_id != server_id {
                return Err(McpError::protocol(
                    "MCP catalog invocation server identity is inconsistent",
                ));
            }
            if state.catalog.completeness != McpCatalogCompleteness::Complete {
                return Err(McpError::config(
                    "MCP catalog invocation catalog is incomplete",
                ));
            }
            if state.status.config_epoch != registry.config_epoch
                || state.status.registry_revision != registry.revision
                || request.expected_config_epoch != state.status.config_epoch
                || request.expected_registry_revision != state.status.registry_revision
                || state.catalog.source_config_epoch != Some(state.status.config_epoch)
                || state.catalog.source_registry_revision != Some(state.status.registry_revision)
                || state.status.config_digest != registry.config_digest
                || state.catalog.source_config_digest.as_ref() != Some(&state.status.config_digest)
                || request.expected_config_digest != state.status.config_digest
            {
                return Err(McpError::config(
                    "MCP catalog invocation configuration snapshot is stale",
                ));
            }
            if request.expected_catalog_generation != state.catalog.generation {
                return Err(McpError::config(
                    "MCP catalog invocation generation is stale",
                ));
            }
            if state.catalog.content_digest.as_ref() != Some(&request.expected_catalog_digest) {
                return Err(McpError::config(
                    "MCP catalog invocation content digest is stale",
                ));
            }
            let tool = state
                .catalog
                .tools
                .iter()
                .find(|tool| tool.id == request.tool_id)
                .ok_or_else(|| McpError::config("MCP catalog invocation tool is unavailable"))?;
            if !tool.routable {
                return Err(McpError::config(
                    "MCP catalog invocation tool is not routable",
                ));
            }
            if tool.raw_name != request.tool_id.raw_name
                || tool.model_name != request.expected_model_name
                || tool.schema_digest != request.expected_schema_digest
            {
                return Err(McpError::config(
                    "MCP catalog invocation tool route is stale",
                ));
            }
            let timeout_ms = request
                .timeout_ms
                .filter(|timeout_ms| *timeout_ms > 0)
                .map(|timeout_ms| timeout_ms.min(registry.config.request_timeout_ms))
                .unwrap_or(registry.config.request_timeout_ms)
                .min(self.inner.policy.security_limits.max_tool_timeout_ms);
            let provenance = McpActiveCallProvenance {
                tool_id: request.tool_id.clone(),
                model_name: request.expected_model_name.clone(),
                config_epoch: request.expected_config_epoch,
                registry_revision: request.expected_registry_revision,
                config_digest: request.expected_config_digest.clone(),
                catalog_generation: request.expected_catalog_generation,
                catalog_digest: request.expected_catalog_digest.clone(),
                schema_digest: request.expected_schema_digest.clone(),
            };
            let call = McpToolCall {
                name: request.tool_id.raw_name.clone(),
                arguments: request.arguments,
                timeout_ms: Some(timeout_ms),
                invocation_id: Some(id.invocation_id),
            };
            if state.active_calls.contains_key(&id) {
                return Err(McpError::config(
                    "MCP active-call identity is already in use",
                ));
            }
            if state.active_calls.len() >= self.inner.policy.max_active_calls_per_server {
                return Err(McpError::capacity(
                    "MCP per-server active-call limit was reached",
                ));
            }
            let global_permit = reserve_global_active_call(&self.inner)?;
            let control = Arc::new(ActiveCallControl::new(id.clone(), provenance, timeout_ms));
            state.active_calls.insert(id, Arc::clone(&control));
            state.status.active_call_count = state.active_calls.len();
            let active_status = state.status.clone();
            (peer, call, control, active_status, global_permit)
        };

        entry.publish_status(&active_status);
        let _active_guard = ActiveCallGuard {
            entry: Arc::clone(&entry),
            control: Arc::clone(&control),
            global_permit: Some(global_permit),
        };
        let final_registry =
            self.inner.registry.get(server_id)?.ok_or_else(|| {
                McpError::config("MCP catalog invocation server is not registered")
            })?;
        if final_registry.config_epoch != request.expected_config_epoch
            || final_registry.revision != request.expected_registry_revision
            || final_registry.config_digest != request.expected_config_digest
            || !final_registry.config.enabled
            || final_registry.config.trust == McpTrustLevel::Untrusted
            || final_registry.config.approval_mode == McpApprovalMode::Deny
        {
            return Err(McpError::config(
                "MCP catalog invocation configuration epoch is stale",
            ));
        }
        {
            let state = lock_entry(&entry)?;
            if state.removed
                || state.status.state != McpServerState::Ready
                || state.status.config_epoch != request.expected_config_epoch
                || state.status.registry_revision != request.expected_registry_revision
                || state.catalog.source_config_epoch != Some(request.expected_config_epoch)
                || state.catalog.source_registry_revision
                    != Some(request.expected_registry_revision)
                || state
                    .active_calls
                    .get(&control.id)
                    .is_none_or(|active| !Arc::ptr_eq(active, &control))
            {
                return Err(McpError::config(
                    "MCP catalog invocation configuration epoch is stale",
                ));
            }
        }
        if self.inner.shutdown_cancel.is_cancelled()
            || cancellation.is_cancelled()
            || control.cancellation.is_cancelled()
        {
            return Err(McpError::cancelled("MCP tools/call"));
        }
        // Cross the dispatch uncertainty boundary only after every Host-owned
        // Registry/Catalog identity and cancellation check has passed. A
        // mutation committed after this point races a possibly-dispatched call
        // and is conservatively settled by the Registry watcher.
        control.dispatch.mark_dispatching();
        // An SDK request handle only proves local queuing, not whether bytes
        // reached the server, so cross the uncertainty boundary immediately
        // before entering the peer.
        control.dispatch.mark_request_queued();
        control.set_state(McpInvocationState::Running);
        let mut protocol_call =
            peer.call_tool_tracked(call, control.cancellation.clone(), control.dispatch.clone());
        let deadline = tokio::time::sleep_until(control.deadline);
        tokio::pin!(deadline);
        let result = tokio::select! {
            biased;
            _ = self.inner.shutdown_cancel.cancelled() => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::Shutdown,
                ).await
            }
            _ = cancellation.cancelled() => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::Cancelled,
                ).await
            }
            _ = control.cancellation.cancelled() => {
                let reason = control.cancellation_reason();
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    reason,
                ).await
            }
            _ = &mut deadline => {
                settle_interrupted_call(
                    &mut protocol_call,
                    &control,
                    self.inner.policy.active_call_settle_timeout,
                    McpOutcomeUnknownReason::TimedOut,
                ).await
            }
            result = &mut protocol_call => normalize_dispatched_result(&control, result),
        };
        let result = result.and_then(|result| {
            validate_tool_result(&result, &self.inner.policy.security_limits)?;
            Ok(result)
        });
        control.set_state(invocation_state_for_result(&result));
        result
    }
}

fn reserve_global_active_call(
    inner: &Arc<ManagerInner>,
) -> Result<GlobalActiveCallPermit, McpError> {
    let maximum = inner.policy.max_active_calls_total;
    let mut current = inner.active_call_count.load(Ordering::Acquire);
    loop {
        if current >= maximum {
            return Err(McpError::capacity(
                "MCP global active-call limit was reached",
            ));
        }
        match inner.active_call_count.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                return Ok(GlobalActiveCallPermit {
                    inner: Arc::clone(inner),
                });
            }
            Err(observed) => current = observed,
        }
    }
}

pub(super) fn is_terminal_invocation_state(state: McpInvocationState) -> bool {
    matches!(
        state,
        McpInvocationState::Completed
            | McpInvocationState::Failed
            | McpInvocationState::Cancelled
            | McpInvocationState::OutcomeUnknown
    )
}

pub(super) fn cancel_active_calls(
    calls: &[Arc<ActiveCallControl>],
    reason: McpOutcomeUnknownReason,
) {
    for call in calls {
        call.cancel(reason);
    }
}

pub(super) async fn settle_active_calls(
    calls: &[Arc<ActiveCallControl>],
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, wait_for_active_call_removal(calls))
        .await
        .is_ok()
}

pub(super) async fn wait_for_active_call_removal(calls: &[Arc<ActiveCallControl>]) {
    for call in calls {
        call.wait_terminal().await;
        call.wait_removed().await;
    }
}

pub(super) fn invocation_state_for_result(
    result: &Result<McpToolResult, McpError>,
) -> McpInvocationState {
    match result {
        Ok(_) => McpInvocationState::Completed,
        Err(error) if error.kind == McpErrorKind::OutcomeUnknown => {
            McpInvocationState::OutcomeUnknown
        }
        Err(error) if error.kind == McpErrorKind::Cancelled => McpInvocationState::Cancelled,
        Err(_) => McpInvocationState::Failed,
    }
}

pub(super) fn normalize_dispatched_result(
    control: &ActiveCallControl,
    result: Result<McpToolResult, McpError>,
) -> Result<McpToolResult, McpError> {
    match result {
        Ok(result) => {
            control.dispatch.mark_response_received();
            Ok(result)
        }
        Err(error) if error.dispatch_certainty_is_authoritative() => {
            if error.dispatch_certainty == Some(McpDispatchCertainty::ResponseReceived) {
                control.dispatch.mark_response_received();
            }
            Err(error)
        }
        Err(error) if error.kind == McpErrorKind::OutcomeUnknown => Err(error),
        Err(error) if error.dispatch_certainty == Some(McpDispatchCertainty::ResponseReceived) => {
            if control.dispatch.certainty() == McpDispatchCertainty::ResponseReceived {
                Err(error)
            } else {
                Err(McpError::outcome_unknown(
                    "MCP tools/call",
                    McpOutcomeUnknownReason::ProtocolFailure,
                    control.dispatch.certainty(),
                ))
            }
        }
        Err(error)
            if control.dispatch.certainty() != McpDispatchCertainty::DefinitelyNotDispatched =>
        {
            let reason = match error.kind {
                McpErrorKind::Cancelled => control.cancellation_reason(),
                McpErrorKind::Timeout => McpOutcomeUnknownReason::TimedOut,
                McpErrorKind::ServerExited => McpOutcomeUnknownReason::ServerExited,
                McpErrorKind::Shutdown => McpOutcomeUnknownReason::Shutdown,
                McpErrorKind::Protocol => McpOutcomeUnknownReason::ProtocolFailure,
                _ => McpOutcomeUnknownReason::TransportClosed,
            };
            Err(McpError::outcome_unknown(
                "MCP tools/call",
                reason,
                control.dispatch.certainty(),
            ))
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn settle_interrupted_call(
    call: &mut BoxMcpFuture<'_, McpToolResult>,
    control: &ActiveCallControl,
    grace: Duration,
    reason: McpOutcomeUnknownReason,
) -> Result<McpToolResult, McpError> {
    control.cancel(reason);
    match tokio::time::timeout(grace, call).await {
        Ok(result) => normalize_dispatched_result(control, result),
        Err(_) => Err(McpError::outcome_unknown(
            "MCP tools/call",
            reason,
            control.dispatch.certainty(),
        )),
    }
}
