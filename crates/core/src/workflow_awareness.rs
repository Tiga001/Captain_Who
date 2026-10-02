//! Read-only model queries. Workflow and mailbox ownership are supplied by the Host.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StateView {
    Members,
    Runtime,
    Configuration,
    #[default]
    All,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateQuery {
    #[serde(default)]
    pub view: StateView,
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MailboxDirection {
    #[default]
    Inbox,
    Outbox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailboxQuery {
    #[serde(default)]
    pub direction: MailboxDirection,
    pub cursor: Option<u64>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_limit")]
    pub limit: usize,
    pub message_id: Option<String>,
    pub input_id: Option<String>,
    pub status: Option<crate::workflow_execution::MailStatus>,
}

fn default_limit() -> usize {
    20
}

fn deserialize_limit<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<usize, D::Error> {
    let value = usize::deserialize(deserializer)?;
    if !(1..=50).contains(&value) {
        return Err(serde::de::Error::custom(
            "Mailbox limit must be between 1 and 50",
        ));
    }
    Ok(value)
}

impl Default for MailboxQuery {
    fn default() -> Self {
        Self {
            direction: MailboxDirection::Inbox,
            cursor: None,
            limit: default_limit(),
            message_id: None,
            input_id: None,
            status: None,
        }
    }
}

fn validate_id(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|value| {
        value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control)
    }) {
        return Err("Invalid organization query identifier".into());
    }
    Ok(())
}

impl StateQuery {
    pub fn validate(&self) -> Result<(), String> {
        validate_id(self.node_id.as_deref())
    }
}

impl MailboxQuery {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=50).contains(&self.limit)
            || self.cursor.is_some_and(|value| value > i64::MAX as u64)
        {
            return Err("Invalid organization mailbox page".into());
        }
        validate_id(self.message_id.as_deref())?;
        validate_id(self.input_id.as_deref())
    }
}

/// Reduce only the model-facing result, after the Host has validated ownership and enriched live
/// state. Storage/monitor projections and the automatic World State remain complete and unchanged.
pub fn state_for_model(
    result: serde_json::Value,
    current_node_id: &str,
    awareness: &serde_json::Value,
    focused: bool,
) -> serde_json::Value {
    crate::world_state::workflow_projection::semantic_state(state_overview_for_model(
        result,
        current_node_id,
        awareness,
        focused,
    ))
}

fn state_overview_for_model(
    mut result: serde_json::Value,
    current_node_id: &str,
    awareness: &serde_json::Value,
    focused: bool,
) -> serde_json::Value {
    use serde_json::{json, Value};
    if result["available"] != true {
        return result;
    }
    // Compare against the exact observation used for this model request, never a fresh baseline
    // that the model has not seen. A different scope must not suppress any live state.
    let same_scope = awareness["available"] == true
        && result["instanceId"].as_str().is_some()
        && result["instanceId"] == awareness["instanceId"]
        && result["executionVersion"].as_str().is_some()
        && result["executionVersion"] == awareness["executionVersion"]
        && result["organizationRevision"] == awareness["organizationRevision"]
        && awareness["currentNodeId"].as_str() == Some(current_node_id)
        && result["currentNodeId"].as_str() == Some(current_node_id);
    if let Some(object) = result.as_object_mut() {
        for key in [
            "workflowName",
            "organizationName",
            "currentNodeId",
            "executionVersion",
            "background",
        ] {
            object.remove(key);
        }
    }
    if let Some(nodes) = result.get_mut("members").and_then(Value::as_array_mut) {
        for node in nodes {
            if node["nodeId"].as_str() != Some(current_node_id) {
                continue;
            }
            if let Some(object) = node.as_object_mut() {
                for key in [
                    "receives",
                    "task",
                    "delivers",
                    "truncatedFields",
                    "detailsQueryHint",
                ] {
                    object.remove(key);
                }
            }
        }
    }
    let Some(runtime) = result.get_mut("runtime").and_then(Value::as_object_mut) else {
        return result;
    };
    if let Some(nodes) = runtime.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            crate::world_state::workflow_projection::remove_duplicate_mail_counts(node);
        }
    }
    runtime.insert(
        "summaryPolicy".into(),
        json!(if focused || !same_scope {
            "complete"
        } else {
            "unchanged_from_world_state_omitted"
        }),
    );
    if focused || !same_scope {
        return result;
    }
    runtime.insert("detailsQueryHint".into(), json!("Omitted overview fields are unchanged from organization.awareness. Nodes absent from that summary and truncated lists remain complete. Query a member by name for its complete current state."));
    let Some(nodes) = runtime.get_mut("nodes").and_then(Value::as_array_mut) else {
        return result;
    };
    for node in nodes {
        let previous = awareness["nodes"].as_array().and_then(|nodes| {
            nodes
                .iter()
                .find(|old| node["nodeId"].as_str().is_some() && old["nodeId"] == node["nodeId"])
        });
        let (Some(previous), Some(object)) = (previous, node.as_object_mut()) else {
            continue;
        };
        for key in [
            "state",
            "paused",
            "waitingForApproval",
            "waitingForInteraction",
            "activeRunId",
            "queuedInputCount",
            "pendingCount",
            "processingCount",
        ] {
            if object
                .get(key)
                .is_some_and(|value| previous.get(key) == Some(value))
            {
                object.remove(key);
            }
        }
        let complete_and_unchanged = object
            .get("currentInputIds")
            .and_then(Value::as_array)
            .is_some_and(|values| {
                previous["currentInputCount"].as_u64() == Some(values.len() as u64)
                    && previous.get("currentInputIds") == object.get("currentInputIds")
            });
        if complete_and_unchanged {
            object.remove("currentInputIds");
        }
        // Keep per-message status; the semantic boundary removes internal run associations.
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn workflow_state_omits_observed_summary_but_preserves_new_state_and_mail_details() {
        let observed = json!({"available":true,"instanceId":"team","executionVersion":"v1","currentNodeId":"a","nodes":[{"nodeId":"b","state":"idle","pendingCount":1,"processingCount":0,"currentInputCount":0,"currentInputIds":[]}]});
        let raw = json!({"available":true,"instanceId":"team","executionVersion":"v1","currentNodeId":"a","background":"already known","members":[{"nodeId":"a","task":"already known"},{"nodeId":"b","task":"new role"}],"runtime":{"nodes":[{"nodeId":"b","state":"idle","pendingCount":2,"processingCount":0,"currentInputCount":0,"currentInputIds":[],"inputs":[{"messageId":"m"}],"inputsTruncated":true}]}});
        let result = state_for_model(raw.clone(), "a", &observed, false);
        assert!(result.get("background").is_none());
        assert!(result["members"][0].get("task").is_none());
        assert_eq!(result["members"][1]["task"], "new role");
        assert!(result["runtime"]["members"][0].get("state").is_none());
        assert_eq!(result["runtime"]["members"][0]["pendingCount"], 2);
        assert_eq!(result["runtime"]["members"][0]["mail"][0]["messageId"], "m");
        assert_eq!(result["runtime"]["members"][0]["mailTruncated"], true);
        let focused = state_for_model(raw, "a", &observed, true);
        assert_eq!(focused["runtime"]["members"][0]["state"], "idle");
    }
    #[test]
    fn workflow_mailbox_query_rejects_removed_states_and_out_of_range_pages() {
        assert!(serde_json::from_value::<MailboxQuery>(json!({"status":"applied"})).is_err());
        assert!(serde_json::from_value::<MailboxQuery>(json!({"limit":0})).is_err());
        assert!(serde_json::from_value::<StateQuery>(json!({"view":"topology"})).is_err());
        assert_eq!(
            serde_json::from_value::<MailboxQuery>(json!({"status":"pending"}))
                .unwrap()
                .status,
            Some(crate::workflow_execution::MailStatus::Pending)
        );
    }
}
