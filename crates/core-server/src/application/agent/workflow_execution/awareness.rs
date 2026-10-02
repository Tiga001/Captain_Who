//! Add live root-Turn attention to the storage-owned workflow projection. A manual user Turn
//! need not have a workflow Input, and approvals can outlive the resident Runtime segment.
use super::*;

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
        let compacting: HashSet<String> = self
            .manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .cloned()
            .collect();
        for node in nodes {
            let Some(conversation_id) = node
                .get("conversationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                continue;
            };
            let active_run = self
                .storage
                .get_in_progress_conversation_turn_identity(&conversation_id)?
                .map(|turn| turn.run_id)
                .or_else(|| {
                    active_turns
                        .get(&conversation_id)
                        .map(|turn| turn.run_id.clone())
                });
            let approval = approvals.contains(&conversation_id);
            let interaction = interactions.contains(&conversation_id);
            node["waitingForApproval"] = json!(approval);
            node["waitingForInteraction"] = json!(interaction);
            node["activeRunId"] = json!(active_run);
            let live_state = if approval {
                Some("waiting_approval")
            } else if interaction {
                Some("waiting_interaction")
            } else if node.get("paused").and_then(Value::as_bool) == Some(true) {
                Some("stopped")
            } else if compacting.contains(&conversation_id) {
                Some("compacting")
            } else if active_run.is_some() {
                Some("running")
            } else {
                None
            };
            if let Some(state) = live_state {
                node["state"] = json!(state);
            }
        }
        Ok(())
    }
}
