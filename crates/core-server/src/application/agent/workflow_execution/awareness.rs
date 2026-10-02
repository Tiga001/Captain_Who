//! Add live root-Turn attention to the storage-owned workflow projection. A manual user Turn
//! need not have a workflow Input, and approvals can outlive the resident Runtime segment.
use super::*;

#[cfg(test)]
mod tests;

impl AgentService {
    pub(super) fn enrich_workflow_awareness(&self, projection: &mut Value) -> Result<(), String> {
        let Some(nodes) = (if projection.get("runtime").is_some() {
            projection
                .get_mut("runtime")
                .and_then(|runtime| runtime.get_mut("nodes"))
        } else {
            projection.get_mut("nodes")
        })
        .and_then(Value::as_array_mut) else {
            // A members-only query has no runtime section to enrich.
            return Ok(());
        };
        let attention = self
            .storage
            .load_human_interaction_attention()
            .map_err(|error| error.to_string())?;
        let mut approvals: HashSet<String> =
            attention.approval_conversation_ids.into_iter().collect();
        // Browser risk approvals are owned by their coordinator rather than pending_actions.
        if let Some(coordinator) = &self.browser_risk_coordinator {
            approvals.extend(
                coordinator
                    .list_pending()
                    .into_iter()
                    .filter(|action| action.status == PendingActionStatus::Pending)
                    .filter_map(|action| action.conversation_id),
            );
        }
        let interactions: HashSet<String> = attention
            .requests
            .into_iter()
            .map(|request| request.conversation_id)
            .collect();
        let active_turns = self
            .active_conversation_turns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        // Durable occupancy can survive restart without a resident worker. It prevents a second
        // Turn from being admitted, but is not evidence that this member is currently running.
        let resident_runs: HashMap<String, String> = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .filter(|(_, control)| matches!(control.steer_state, ActiveRunSteerState::Accepting))
            .map(|(run_id, control)| (control.conversation_id.clone(), run_id.clone()))
            .collect();
        let compacting: HashSet<String> = self
            .manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .cloned()
            .collect();
        for node in nodes {
            node["waitingForApproval"] = json!(false);
            node["waitingForInteraction"] = json!(false);
            node["hasPendingInteraction"] = json!(false);
            node["activeRunId"] = Value::Null;
            node["bindingAvailable"] = json!(false);
            let Some(conversation_id) = node
                .get("conversationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                node["state"] = json!("unknown");
                continue;
            };
            if self
                .storage
                .load_conversation_meta(&conversation_id)?
                .is_none()
            {
                node["state"] = json!("unknown");
                continue;
            }
            node["bindingAvailable"] = json!(true);
            let active_run = self
                .storage
                .get_in_progress_conversation_turn_identity(&conversation_id)?
                .map(|turn| turn.run_id)
                .or_else(|| {
                    active_turns
                        .get(&conversation_id)
                        .map(|turn| turn.run_id.clone())
                })
                .or_else(|| resident_runs.get(&conversation_id).cloned());
            let approval = approvals.contains(&conversation_id);
            let pending_interaction = interactions.contains(&conversation_id);
            let interaction = active_run
                .as_deref()
                .map(|run_id| self.storage.is_sync_human_interaction_run_waiting(run_id))
                .transpose()
                .map_err(|error| error.to_string())?
                .unwrap_or(false);
            node["waitingForApproval"] = json!(approval);
            node["waitingForInteraction"] = json!(interaction);
            node["hasPendingInteraction"] = json!(pending_interaction);
            node["activeRunId"] = json!(active_run);
            let live_state = if approval {
                "waiting_approval"
            } else if interaction {
                "waiting_interaction"
            } else if node.get("paused").and_then(Value::as_bool) == Some(true) {
                "stopped"
            } else if compacting.contains(&conversation_id) {
                "compacting"
            } else if active_run
                .as_ref()
                .is_some_and(|run_id| resident_runs.get(&conversation_id) == Some(run_id))
            {
                "running"
            } else if active_run.is_some()
                || node
                    .get("processingCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    > 0
            {
                "unknown"
            } else if node
                .get("pendingCount")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
            {
                "queued"
            } else {
                "idle"
            };
            // Always replace the storage estimate, including idle. A previous failure or old
            // in_progress record describes history, not the state of the current worker.
            node["state"] = json!(live_state);
        }
        Ok(())
    }
}
