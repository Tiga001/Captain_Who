//! Trusted Host ports for cross-conversation workflow collaboration.
//!
//! None of these execution identities are deserializable model arguments. A workflow snapshot
//! describes capabilities; admission still rechecks current bindings in the Host transaction.
use crate::workflow_awareness::{MailboxQuery, StateQuery};
use crate::workflow_execution::{
    ConversationSnapshot as WorkflowConversationSnapshot, SendOutput as WorkflowSendOutput,
    SendReceipt as WorkflowSendReceipt,
};
use crate::{AgentResult, AgentSamplingBoundaryRequest};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct WorkflowSendInvocation {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub execution_version: String,
    pub messages: Vec<WorkflowSendOutput>,
}

#[derive(Debug, Clone)]
pub struct WorkflowMutationInvocation {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub execution_version: String,
    pub action: crate::workflow_execution::MutationAction,
    pub message_ids: Vec<String>,
}

/// Keep renderer receipts rich, but never repeat accepted letter bodies in model tool results.
/// Accepted letters enter once through the trusted WorkflowDelivery sampling boundary instead.
pub fn workflow_mutation_model_projection(
    result: &crate::AgentToolResult,
) -> crate::AgentToolResult {
    let mut projected = result.clone();
    if let Some(value) = projected.result.as_mut() {
        if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                if let Some(fields) = message.as_object_mut() {
                    fields.remove("content");
                    fields.remove("body");
                }
            }
        }
    }
    projected
}

pub trait WorkflowRuntimeHost: Send + Sync {
    fn snapshot(&self) -> AgentResult<Option<WorkflowConversationSnapshot>>;
    fn send(&self, invocation: WorkflowSendInvocation) -> AgentResult<WorkflowSendReceipt>;
    fn mutate(&self, _invocation: WorkflowMutationInvocation) -> AgentResult<Value> {
        Err(crate::AgentError::new(
            "This Host cannot change workflow mail.",
        ))
    }
    /// Read projections are scoped to the Host-bound independent conversation and admitted run.
    /// They never claim inputs, acknowledge deliveries or wake a recipient.
    fn state(&self, query: StateQuery) -> AgentResult<Value>;
    fn mailbox(&self, query: MailboxQuery) -> AgentResult<Value>;
    /// Live, compact observation refreshed at each normal model sampling boundary. Unlike the
    /// admitted identity, this must not be persisted in the frozen run snapshot.
    fn awareness(&self) -> AgentResult<Value>;
}

/// A complete input already claimed durably for this conversation, run and sampling boundary.
/// The content is assembled by the Host from verified sender records and the target snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorkflowDelivery {
    pub trace_sequence: u64,
    pub input_id: String,
    pub instance_id: String,
    pub workflow_name: String,
    pub content: String,
    pub created_at: i64,
}

pub trait AgentWorkflowInbox: Send + Sync {
    fn bind_for_model_batch(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<AgentWorkflowDelivery>>;
}

/// Derive a short new-mail notification from the last state actually observed by a Provider.
/// Preparation, previews and mailbox reads never acknowledge arrivals. The existing CWS journal
/// records this projection at the exact request boundary and confirms it only after a response.
pub fn workflow_mailbox_sections_for_request(
    sections: Vec<crate::WorldStateSectionEnvelope>,
    records: &[crate::AnchoredWorldStateRecord],
) -> AgentResult<Vec<crate::WorldStateSectionEnvelope>> {
    let mut observed = None;
    for entry in records.iter().take_while(|entry| entry.model_observed) {
        observed = Some(match (&entry.record, observed) {
            (crate::WorldStateRecord::Full(snapshot), _) => snapshot.clone(),
            (crate::WorldStateRecord::Diff(diff), Some(snapshot)) => {
                crate::WorldStateReducer::fold(snapshot, std::slice::from_ref(diff))
                    .map_err(|e| crate::AgentError::new(e.to_string()))?
            }
            (crate::WorldStateRecord::Diff(_), None) => {
                return Err(crate::AgentError::new(
                    "Observed World State has no base snapshot.",
                ))
            }
        });
    }
    let section_id = crate::WorldStateSectionId::extension("workflow.mailbox")
        .map_err(|e| crate::AgentError::new(e.to_string()))?;
    let previous = observed
        .as_ref()
        .and_then(|snapshot| snapshot.section(&section_id))
        .map(|section| &section.state);
    sections.into_iter().map(|section| {
        if section.id != section_id || section.state["available"] != true { return Ok(section); }
        let current = &section.state;
        let previous = previous.filter(|old| old["available"] == true
            && old["instanceId"] == current["instanceId"]
            && old["nodeId"] == current["nodeId"]);
        let previous_count = previous.and_then(|v| v["mailbox"]["receivedCount"].as_u64()).unwrap_or(0);
        let previous_sequence = previous.and_then(|v| v["mailbox"]["latestSequence"].as_u64()).unwrap_or(0);
        let mailbox = &current["mailbox"];
        let new_count = mailbox["receivedCount"].as_u64().unwrap_or(0).saturating_sub(previous_count);
        let mut projection = serde_json::json!({"available":true,
            "pendingCount":mailbox["pendingCount"].as_u64().unwrap_or(0),
            "processingCount":mailbox["processingCount"].as_u64().unwrap_or(0),
            "newMessageCount":new_count});
        if new_count > 0 {
            let sources: std::collections::BTreeSet<_> = mailbox["recentArrivals"].as_array()
                .into_iter().flatten()
                .filter(|arrival| arrival["sequence"].as_u64().unwrap_or(0)>previous_sequence)
                .filter_map(|arrival| arrival["sourceNodeName"].as_str()).take(3).collect();
            projection["recentSources"] = serde_json::json!(sources);
            projection["notice"] = serde_json::json!(format!("Your workflow inbox received {new_count} new message(s). Previewing mail does not accept it; use workflow_accept to handle pending letters in this run."));
        }
        crate::WorldStateSectionEnvelope::model_visible(section.id,section.lifetime,section.state,projection)
            .map_err(|e| crate::AgentError::new(e.to_string()))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn mailbox(received: u64, latest: u64, pending: u64) -> crate::WorldStateSectionEnvelope {
        let value = json!({"available":true,"instanceId":"w","executionVersion":"v","nodeId":"n",
            "mailbox":{"receivedCount":received,"latestSequence":latest,"pendingCount":pending,"processingCount":1,
            "recentArrivals":[{"sequence":latest,"messageId":"mail","sourceNodeId":"peer","sourceNodeName":"Peer"}]}});
        crate::WorldStateSectionEnvelope::model_visible(
            crate::WorldStateSectionId::extension("workflow.mailbox").unwrap(),
            crate::WorldStateLifetime::Conversation,
            value.clone(),
            value,
        )
        .unwrap()
    }
    fn observed(
        section: crate::WorldStateSectionEnvelope,
        seen: bool,
    ) -> crate::AnchoredWorldStateRecord {
        let mut record = crate::AnchoredWorldStateRecord::new(
            crate::WorldStateRecord::Full(
                crate::WorldStateSnapshot::new("epoch", 0, vec![section]).unwrap(),
            ),
            None,
        )
        .unwrap();
        record.model_observed = seen;
        record
    }
    #[test]
    fn new_mail_notice_uses_arrival_count_not_pending_delta_or_global_sequence_distance() {
        let previous = observed(mailbox(7, 24, 2), true);
        let projected =
            workflow_mailbox_sections_for_request(vec![mailbox(8, 92, 2)], &[previous]).unwrap();
        let value = projected[0].model_projection.as_ref().unwrap();
        assert_eq!(value["newMessageCount"], 1);
        assert_eq!(value["pendingCount"], 2);
        assert_eq!(value["recentSources"], json!(["Peer"]));
        assert!(value.get("latestSequence").is_none());
        assert!(value.get("receivedCount").is_none());
        assert!(value.get("messageId").is_none());
        assert_eq!(projected[0].state["mailbox"]["latestSequence"], 92);
    }
    #[test]
    fn preparing_or_previewing_mail_does_not_consume_notification() {
        let raw = mailbox(3, 25, 3);
        let first = workflow_mailbox_sections_for_request(vec![raw.clone()], &[]).unwrap();
        let failed_request = observed(first[0].clone(), false);
        let repeated =
            workflow_mailbox_sections_for_request(vec![raw.clone()], &[failed_request]).unwrap();
        assert_eq!(
            repeated[0].model_projection.as_ref().unwrap()["newMessageCount"],
            3
        );
        let confirmed = observed(first[0].clone(), true);
        let next = workflow_mailbox_sections_for_request(vec![raw], &[confirmed]).unwrap();
        assert_eq!(
            next[0].model_projection.as_ref().unwrap()["newMessageCount"],
            0
        );
        assert!(next[0]
            .model_projection
            .as_ref()
            .unwrap()
            .get("notice")
            .is_none());
    }
    #[test]
    fn editing_configuration_does_not_reannounce_existing_mail() {
        let previous = observed(mailbox(4, 27, 2), true);
        let mut changed = mailbox(4, 27, 2);
        changed.state["executionVersion"] = json!("new-configuration");
        let next = workflow_mailbox_sections_for_request(vec![changed], &[previous]).unwrap();
        assert_eq!(
            next[0].model_projection.as_ref().unwrap()["newMessageCount"],
            0
        );
    }
    #[test]
    fn later_arrival_is_not_acknowledged_by_previous_response_and_scope_does_not_leak() {
        let previous = observed(mailbox(3, 25, 3), true);
        let next = workflow_mailbox_sections_for_request(
            vec![mailbox(4, 27, 4)],
            std::slice::from_ref(&previous),
        )
        .unwrap();
        assert_eq!(
            next[0].model_projection.as_ref().unwrap()["newMessageCount"],
            1
        );
        let mut another = mailbox(2, 3, 2);
        let mut state = another.state.clone();
        state["instanceId"] = json!("other");
        another = crate::WorldStateSectionEnvelope::model_visible(
            another.id,
            another.lifetime,
            state.clone(),
            state,
        )
        .unwrap();
        let next = workflow_mailbox_sections_for_request(vec![another], &[previous]).unwrap();
        assert_eq!(
            next[0].model_projection.as_ref().unwrap()["newMessageCount"],
            2
        );
    }
    #[test]
    fn model_mail_mutation_receipt_does_not_repeat_letter_body_but_renderer_can_expand() {
        let raw = crate::AgentToolResult {
            call_id: "call".into(),
            tool: "workflow_accept".into(),
            ok: true,
            result: Some(
                json!({"messages":[{"messageId":"m","success":true,"status":"processing","content":"BODY","sourceNodeName":"Peer"}]}),
            ),
            error: None,
            exact_archive_file: None,
        };
        let model = workflow_mutation_model_projection(&raw);
        assert!(model.result.as_ref().unwrap()["messages"][0]
            .get("content")
            .is_none());
        assert_eq!(
            model.result.as_ref().unwrap()["messages"][0]["messageId"],
            "m"
        );
        assert_eq!(
            raw.result.as_ref().unwrap()["messages"][0]["content"],
            "BODY"
        );
        assert_eq!(
            serde_json::to_value(crate::tools::model_projection_for_persisted_continuation(
                &raw
            ))
            .unwrap(),
            serde_json::to_value(model).unwrap()
        );
    }
}
