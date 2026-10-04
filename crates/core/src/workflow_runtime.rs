//! Trusted Host ports for cross-conversation organization collaboration.
//!
//! None of these execution identities are deserializable model arguments. An organization snapshot
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
    /// Original semantic arguments, used for durable replay before resolving names again.
    pub model_input: Option<Value>,
    pub recipient_versions: std::collections::BTreeMap<String, String>,
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

/// Host-owned lookup for a previously committed mail call. It cannot perform a new mutation.
#[derive(Debug, Clone)]
pub struct WorkflowMailReceiptQuery {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub call: WorkflowMailReceiptCall,
}

#[derive(Debug, Clone)]
pub enum WorkflowMailReceiptCall {
    SemanticSend {
        input: Value,
    },
    Send {
        messages: Vec<WorkflowSendOutput>,
    },
    Mutation {
        action: crate::workflow_execution::MutationAction,
        message_ids: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub enum WorkflowMailReceipt {
    Send(WorkflowSendReceipt),
    Mutation(Value),
}

#[derive(Debug, Clone)]
pub struct OrganizationEditInvocation {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub execution_version: String,
    pub expected_revision: u64,
    pub input: crate::organization_personnel::Input,
    pub model_input: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct OrganizationEditReceiptQuery {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub input: Value,
}

/// Rename only structured, Host-owned result metadata. Mail bodies and user-assigned names are
/// opaque content and are never rewritten; persistence retains its established wire schema.
pub(crate) fn organization_result_projection(mut value: Value) -> Value {
    fn rename_metadata(value: &mut Value) {
        if let Some(fields) = value.as_object_mut() {
            if let Some(name) = fields.remove("workflowName") {
                fields.entry("organizationName").or_insert(name);
            }
        }
    }
    rename_metadata(&mut value);
    if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages {
            rename_metadata(message);
        }
    }
    value
}

/// Model mail results use sender/recipient names and exactly one durable message identifier.
/// Renderer and stored receipts retain all routing identities and complete expansion data.
pub fn organization_mail_model_projection(
    result: &crate::AgentToolResult,
) -> crate::AgentToolResult {
    let mut projected = result.clone();
    if let Some(value) = projected.result.as_mut() {
        *value = organization_result_projection(std::mem::take(value));
        if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
            for message in messages {
                if let Some(fields) = message.as_object_mut() {
                    if let Some(id) = fields.remove("id") {
                        fields.entry("messageId").or_insert(id);
                    }
                    for key in ["organizationName", "runStatus"] {
                        fields.remove(key);
                    }
                    if fields.get("bodyAvailable") == Some(&Value::Bool(true)) {
                        fields.remove("bodyAvailable");
                    }
                }
            }
        }
        *value = crate::world_state::workflow_projection::semantic_state(std::mem::take(value));
    }
    projected
}

/// Keep renderer receipts rich, but never repeat accepted letter bodies in model tool results.
/// Accepted letters enter once through the trusted WorkflowDelivery sampling boundary instead.
pub fn workflow_mutation_model_projection(
    result: &crate::AgentToolResult,
) -> crate::AgentToolResult {
    let mut projected = organization_mail_model_projection(result);
    if let Some(messages) = projected
        .result
        .as_mut()
        .and_then(|value| value.get_mut("messages"))
        .and_then(Value::as_array_mut)
    {
        for message in messages {
            if let Some(fields) = message.as_object_mut() {
                fields.remove("content");
                fields.remove("body");
            }
        }
    }
    projected
}

/// The rich receipt supplies UI links and before/after diffs; model history only needs names and
/// the changed settings. Long role text is already present in the request and is not repeated.
pub fn organization_edit_model_projection(
    result: &crate::AgentToolResult,
) -> crate::AgentToolResult {
    let mut projected = result.clone();
    let Some(value) = projected.result.as_mut() else {
        return projected;
    };
    if let Some(fields) = value.as_object_mut() {
        for key in [
            "instanceId",
            "organizationRevision",
            "affectedConversationIds",
        ] {
            fields.remove(key);
        }
    }
    if let Some(changes) = value.get_mut("changes").and_then(Value::as_array_mut) {
        for change in changes {
            let Some(object) = change.as_object_mut() else {
                continue;
            };
            object.remove("entityId");
            object.remove("conversationId");
            let entity_type = object.remove("entityType");
            if let Some(name) = object.remove("entityName") {
                object.insert(
                    if entity_type.as_ref().and_then(Value::as_str) == Some("department") {
                        "department"
                    } else {
                        "member"
                    }
                    .into(),
                    name,
                );
            }
            if let Some(fields) = object.get_mut("fields").and_then(Value::as_array_mut) {
                for field in fields {
                    let Some(field) = field.as_object_mut() else {
                        continue;
                    };
                    field.remove("before");
                    field.remove("beforeLabel");
                    let name = field
                        .get("field")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let label = field.remove("afterLabel");
                    match name.as_str() {
                        "task" | "receives" | "delivers" => {
                            field.remove("after");
                        }
                        "departmentId" | "parentId" | "modelConfigId" => {
                            field.insert(
                                "field".into(),
                                Value::String(
                                    match name.as_str() {
                                        "departmentId" => "department",
                                        "parentId" => "parentDepartment",
                                        _ => "model",
                                    }
                                    .into(),
                                ),
                            );
                            // Never fall back to an opaque ID if an old model/department disappeared.
                            field.insert("after".into(), label.unwrap_or(Value::Null));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    projected
}

/// State results already carry the request-relative overview policy. Apply the same semantic
/// boundary again on persisted replay so no internal identity reappears after recovery.
pub fn organization_state_model_projection(
    result: &crate::AgentToolResult,
) -> crate::AgentToolResult {
    let mut projected = result.clone();
    if let Some(value) = projected.result.as_mut() {
        *value = crate::world_state::workflow_projection::semantic_state(std::mem::take(value));
    }
    projected
}

pub trait WorkflowRuntimeHost: Send + Sync {
    fn snapshot(&self) -> AgentResult<Option<WorkflowConversationSnapshot>>;
    fn send(&self, invocation: WorkflowSendInvocation) -> AgentResult<WorkflowSendReceipt>;
    fn mutate(&self, _invocation: WorkflowMutationInvocation) -> AgentResult<Value> {
        Err(crate::AgentError::new(
            "This Host cannot change organization mail.",
        ))
    }
    fn mail_receipt(
        &self,
        _query: WorkflowMailReceiptQuery,
    ) -> AgentResult<Option<WorkflowMailReceipt>> {
        Ok(None)
    }
    fn edit_organization(
        &self,
        _invocation: OrganizationEditInvocation,
    ) -> AgentResult<crate::organization_personnel::Receipt> {
        Err(crate::AgentError::new(
            "This Host cannot edit this organization.",
        ))
    }
    fn organization_edit_receipt(
        &self,
        _query: OrganizationEditReceiptQuery,
    ) -> AgentResult<Option<crate::organization_personnel::Receipt>> {
        Ok(None)
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
    let section_id = crate::WorldStateSectionId::extension("organization.mailbox")
        .map_err(|e| crate::AgentError::new(e.to_string()))?;
    let previous = observed
        .as_ref()
        .and_then(|snapshot| snapshot.section(&section_id))
        .map(|section| &section.state);
    sections
        .into_iter()
        .map(|section| {
            if section.id != section_id || section.state["available"] != true {
                return Ok(section);
            }
            let current = &section.state;
            let previous = previous.filter(|old| {
                old["available"] == true
                    && old["instanceId"] == current["instanceId"]
                    && old["nodeId"] == current["nodeId"]
            });
            let previous_count = previous
                .and_then(|v| v["mailbox"]["receivedCount"].as_u64())
                .unwrap_or(0);
            let previous_sequence = previous
                .and_then(|v| v["mailbox"]["latestSequence"].as_u64())
                .unwrap_or(0);
            let mailbox = &current["mailbox"];
            let new_count = mailbox["receivedCount"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(previous_count);
            let mut projection = serde_json::json!({"available":true,
            "pendingCount":mailbox["pendingCount"].as_u64().unwrap_or(0),
            "processingCount":mailbox["processingCount"].as_u64().unwrap_or(0),
            "newMessageCount":new_count});
            if new_count > 0 {
                let sources: std::collections::BTreeSet<_> = mailbox["recentArrivals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|arrival| arrival["sequence"].as_u64().unwrap_or(0) > previous_sequence)
                    .filter_map(|arrival| arrival["sourceNodeName"].as_str())
                    .take(3)
                    .collect();
                projection["recentSources"] = serde_json::json!(sources);
                projection["notice"] = serde_json::json!(format!(
                    "Your organization inbox received {new_count} new message(s)."
                ));
            }
            crate::WorldStateSectionEnvelope::model_visible(
                section.id,
                section.lifetime,
                section.state,
                projection,
            )
            .map_err(|e| crate::AgentError::new(e.to_string()))
        })
        .collect()
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
            crate::WorldStateSectionId::extension("organization.mailbox").unwrap(),
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
            tool: "organization_accept".into(),
            ok: true,
            result: Some(
                json!({"messages":[{"messageId":"m","success":true,"status":"processing","content":"BODY","sourceNodeName":"Peer"}]}),
            ),
            error: None,
            exact_archive_file: None,
        };
        for name in [
            "organization_accept",
            "organization_complete",
            "organization_recall",
        ] {
            let mut receipt = raw.clone();
            receipt.tool = name.into();
            let projected = crate::tools::model_projection_for_persisted_continuation(&receipt);
            assert!(
                projected.result.unwrap()["messages"][0]
                    .get("content")
                    .is_none(),
                "{name}"
            );
        }
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
    #[test]
    fn model_mail_history_index_preserves_individual_lookup_and_separate_pagination() {
        let raw = crate::AgentToolResult {
            call_id: "mail-overview".into(),
            tool: "organization_get_mailbox".into(),
            ok: true,
            result: Some(json!({
                "instanceId":"internal-org", "workflowName":"Writing team", "direction":"inbox", "view":"overview",
                "counts":{"total":24,"pending":2,"processing":1,"processed":18,"stopped":1,"failed":1,"recalled":1},
                "countsScope":"entire_selected_mailbox",
                "messages":[{"messageId":"pending-1","sourceNodeName":"Boss","status":"pending","content":"Please review","bodyAvailable":true}],
                "nextCursor":10,
                "history":{"total":21,"nextCursor":8,"messages":[{
                    "messageId":"history-1", "sourceNodeName":"Editor", "sourceConversationId":"internal-source",
                    "status":"processed", "createdAt":123,
                    "bodyRetrieval":{"tool":"organization_get_mailbox","arguments":{"direction":"inbox","messageId":"history-1"}}
                }]}
            })),
            error: None,
            exact_archive_file: None,
        };
        let projected = organization_mail_model_projection(&raw);
        let value = projected.result.as_ref().unwrap();
        assert_eq!(value["messages"][0]["content"], "Please review");
        assert_eq!(value["history"]["messages"][0]["from"], "Editor");
        assert_eq!(value["history"]["messages"][0]["messageId"], "history-1");
        assert_eq!(
            value["history"]["messages"][0]["bodyRetrieval"],
            json!({
                "tool":"organization_get_mailbox","arguments":{"direction":"inbox","messageId":"history-1"}
            })
        );
        assert!(value["history"]["messages"][0].get("content").is_none());
        assert_eq!(value["counts"]["processed"], 18);
        assert_eq!(value["countsScope"], "entire_selected_mailbox");
        assert_eq!(value["nextCursor"], 10);
        assert_eq!(value["history"]["total"], 21);
        assert_eq!(value["history"]["nextCursor"], 8);
        assert!(!value.to_string().contains("internal-"));
        assert_eq!(
            serde_json::to_value(crate::tools::model_projection_for_persisted_continuation(
                &raw
            ))
            .unwrap(),
            serde_json::to_value(&projected).unwrap()
        );
        assert_eq!(
            serde_json::to_value(organization_mail_model_projection(&projected)).unwrap(),
            serde_json::to_value(projected).unwrap()
        );
    }

    #[test]
    fn model_mail_history_preserves_bodies_and_message_identity_without_routing_ids() {
        let mut raw = crate::AgentToolResult {
            call_id: "tool-call".into(),
            tool: "organization_get_mailbox".into(),
            ok: true,
            result: Some(
                json!({"instanceId":"internal-org","deliveryId":"internal-delivery","nodeId":"internal-self","workflowName":"Writing team","direction":"inbox","nextCursor":3,
                "messages":[{"id":"mail-1","messageId":"mail-1","sourceNodeId":"internal-sender","sourceNodeName":"Boss","targetNodeName":"人事负责人","targetNodeId":"internal-target",
                    "sourceConversationId":"internal-source-chat","targetConversationId":"internal-target-chat","sourceConversationTitle":"Private title",
                    "inputId":"internal-input","runId":"internal-run","deliveryId":"internal-delivery","membershipVersion":"internal-incarnation",
                    "status":"pending","content":"literal nodeId and workflowName are part of this message","createdAt":123,"sequence":4,"bodyAvailable":true}]}),
            ),
            error: None,
            exact_archive_file: None,
        };
        let projected = organization_mail_model_projection(&raw);
        let value = projected.result.as_ref().unwrap();
        assert_eq!(value["messages"][0]["messageId"], "mail-1");
        assert_eq!(value["messages"][0]["from"], "Boss");
        assert_eq!(value["messages"][0]["to"], "人事负责人");
        assert_eq!(
            value["messages"][0]["content"],
            raw.result.as_ref().unwrap()["messages"][0]["content"]
        );
        assert_eq!(value["nextCursor"], 3);
        assert!(!value.to_string().contains("internal-"));
        assert!(!value.to_string().contains("Private title"));
        assert!(value["messages"][0].get("id").is_none());
        for name in ["organization_send", "organization_get_mailbox"] {
            raw.tool = name.into();
            let model = organization_mail_model_projection(&raw);
            assert_eq!(
                serde_json::to_value(crate::tools::model_projection_for_persisted_continuation(
                    &raw
                ))
                .unwrap(),
                serde_json::to_value(&model).unwrap()
            );
            assert_eq!(
                serde_json::to_value(organization_mail_model_projection(&model)).unwrap(),
                serde_json::to_value(model).unwrap()
            );
        }
        assert!(raw
            .result
            .unwrap()
            .to_string()
            .contains("internal-source-chat"));
    }

    #[test]
    fn model_edit_receipt_reports_names_and_settings_while_ui_keeps_detailed_diff() {
        let raw = crate::AgentToolResult {
            call_id: "tool-call".into(),
            tool: "organization_edit".into(),
            ok: true,
            result: Some(
                json!({"instanceId":"internal-org","organizationRevision":4,"organizationName":"Writing team","affectedConversationIds":["internal-chat"],
                "changes":[{"action":"update_member","entityType":"member","entityId":"internal-member","entityName":"周宁","conversationId":"internal-chat",
                    "fields":[{"field":"departmentId","before":"internal-old-dept","after":"internal-new-dept","beforeLabel":"旧部门","afterLabel":"人事部/薪酬组"},
                        {"field":"modelConfigId","before":"internal-model-old","after":"internal-model-new","afterLabel":"Writing assistant"},
                        {"field":"task","before":"old long role","after":"new long role"},{"field":"rank","before":1,"after":2}]}]}),
            ),
            error: None,
            exact_archive_file: None,
        };
        let projected = organization_edit_model_projection(&raw);
        let value = projected.result.as_ref().unwrap();
        assert_eq!(value["changes"][0]["member"], "周宁");
        assert_eq!(value["changes"][0]["fields"][0]["after"], "人事部/薪酬组");
        assert_eq!(
            value["changes"][0]["fields"][1]["after"],
            "Writing assistant"
        );
        assert!(value["changes"][0]["fields"][2].get("after").is_none());
        assert_eq!(value["changes"][0]["fields"][3]["after"], 2);
        assert!(!value.to_string().contains("internal-"));
        assert_eq!(
            serde_json::to_value(crate::tools::model_projection_for_persisted_continuation(
                &raw
            ))
            .unwrap(),
            serde_json::to_value(&projected).unwrap()
        );
        assert_eq!(
            serde_json::to_value(organization_edit_model_projection(&projected)).unwrap(),
            serde_json::to_value(projected).unwrap()
        );
        assert!(raw.result.unwrap().to_string().contains("old long role"));
    }
}
