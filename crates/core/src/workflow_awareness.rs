//! Read-only model queries. Workflow and mailbox ownership are supplied by the Host.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StateView {
    #[default]
    Overview,
    Structure,
    Members,
    Runtime,
    Configuration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateQuery {
    #[serde(default)]
    pub view: StateView,
    pub node_id: Option<String>,
    pub department_id: Option<String>,
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    pub search: Option<String>,
    pub status: Option<String>,
    pub cursor: Option<usize>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_limit")]
    pub limit: usize,
    #[serde(default)]
    pub include_mail: bool,
    pub mail_cursor: Option<u64>,
}

fn default_true() -> bool {
    true
}
impl Default for StateQuery {
    fn default() -> Self {
        Self {
            view: StateView::Overview,
            node_id: None,
            department_id: None,
            include_descendants: true,
            search: None,
            status: None,
            cursor: None,
            limit: default_limit(),
            include_mail: false,
            mail_cursor: None,
        }
    }
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
            "Query limit must be between 1 and 50",
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
        validate_id(self.node_id.as_deref())?;
        validate_id(self.department_id.as_deref())?;
        if !(1..=50).contains(&self.limit) || self.cursor.is_some_and(|v| v > i64::MAX as usize) {
            return Err("Invalid organization state page".into());
        }
        if self.search.as_deref().is_some_and(|s| {
            s.trim().is_empty() || s.chars().count() > 512 || s.chars().any(char::is_control)
        }) {
            return Err(
                "Search must contain a name or responsibility keyword (at most 512 characters)"
                    .into(),
            );
        }
        if self
            .status
            .as_deref()
            .is_some_and(|s| !STATE_STATUSES.contains(&s))
        {
            return Err("Invalid organization runtime status".into());
        }
        if self.status.is_some() && self.view != StateView::Runtime {
            return Err("status is only supported by the runtime view".into());
        }
        if self.include_mail && (self.view != StateView::Runtime || self.node_id.is_none()) {
            return Err("includeMail requires the runtime view and a specific member".into());
        }
        if self
            .mail_cursor
            .is_some_and(|cursor| cursor > i64::MAX as u64)
            || (self.mail_cursor.is_some() && !self.include_mail)
        {
            return Err("mailCursor requires includeMail and a valid mailbox sequence".into());
        }
        if matches!(self.view, StateView::Overview | StateView::Structure)
            && (self.node_id.is_some() || self.search.is_some())
        {
            return Err("Use members, runtime or configuration to select or search members".into());
        }
        if self.view == StateView::Overview && self.cursor.is_some() {
            return Err(
                "Overview is an aggregate; use runtime to page through members needing attention"
                    .into(),
            );
        }
        Ok(())
    }
}

const STATE_STATUSES: &[&str] = &[
    "idle",
    "running",
    "queued",
    "stopped",
    "waiting_approval",
    "waiting_interaction",
    "compacting",
    "unknown",
];

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

/// Finalize only after the Host has enriched every runtime candidate. The automatic World State
/// is deliberately not used as a deduplication baseline: an explicit query returns complete facts.
pub fn state_for_model(mut result: serde_json::Value, query: &StateQuery) -> serde_json::Value {
    use serde_json::json;
    if result["available"] != true {
        return result;
    }
    if query.view == StateView::Overview {
        let nodes = result["runtime"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut counts = serde_json::Map::new();
        for status in STATE_STATUSES {
            counts.insert((*status).into(), json!(0));
        }
        let (mut pending, mut processing) = (0_u64, 0_u64);
        let mut attention = Vec::new();
        for node in &nodes {
            let state = node["state"]
                .as_str()
                .filter(|s| STATE_STATUSES.contains(s))
                .unwrap_or("unknown");
            counts[state] = json!(counts[state].as_u64().unwrap_or(0) + 1);
            pending += node["pendingCount"].as_u64().unwrap_or(0);
            processing += node["processingCount"].as_u64().unwrap_or(0);
            let mut reasons = Vec::new();
            if state == "stopped" {
                reasons.push("stopped");
            }
            if node["latestRun"]["status"] == "failed"
                && !matches!(
                    state,
                    "running" | "waiting_approval" | "waiting_interaction" | "compacting"
                )
            {
                reasons.push("last_run_failed");
            }
            if !reasons.is_empty() {
                attention.push(json!({"nodeId":node["nodeId"],"nodeName":node["nodeName"],
                    "departmentId":node["departmentId"],"state":state,"reasons":reasons,
                    "pendingCount":node["pendingCount"],"latestRun":node["latestRun"]}));
            }
        }
        let total = attention.len();
        attention.truncate(query.limit);
        result["overview"] = json!({"memberCount":nodes.len(),
            "departmentCount":result["scope"]["departmentCount"],"stateCounts":counts,
            "mailBacklog":{"pendingCount":pending,"processingCount":processing},
            "attention":{"members":attention,"total":total,"returned":total.min(query.limit),
                "truncated":total>query.limit,"availability":if total>query.limit {"truncated"} else {"complete"},
                "followUp":"Use runtime for current member details; status=stopped filters stopped members. Runtime includes each member's latest run result."}});
        result.as_object_mut().unwrap().remove("runtime");
    } else {
        let (items, page_target) = match query.view {
            StateView::Members => (result.get("members").cloned(), "members"),
            StateView::Structure => (result["structure"].get("departments").cloned(), "structure"),
            StateView::Runtime => (result["runtime"].get("nodes").cloned(), "runtime"),
            StateView::Configuration => (
                result["configuration"].get("members").cloned(),
                "configuration",
            ),
            StateView::Overview => unreachable!(),
        };
        let mut items = items
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        if query.view == StateView::Runtime {
            items.retain(|node| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|status| node["state"] == status)
            });
        }
        let total = items.len();
        let cursor = query.cursor.unwrap_or(0);
        let page = items
            .into_iter()
            .skip(cursor)
            .take(query.limit)
            .collect::<Vec<_>>();
        let returned = page.len();
        let next = cursor.saturating_add(returned);
        let has_more = next < total;
        result["page"] = json!({"total":total,"returned":returned,"cursor":cursor,"limit":query.limit,
            "nextCursor":if has_more {Some(next)} else {None},"truncated":has_more,
            "availability":if has_more {"truncated"} else {"complete"},
            "empty":returned==0});
        match page_target {
            "members" => result["members"] = json!(page),
            "structure" => result["structure"]["departments"] = json!(page),
            "runtime" => result["runtime"]["nodes"] = json!(page),
            "configuration" => result["configuration"]["members"] = json!(page),
            _ => unreachable!(),
        }
    }
    // Keep the minimal directory until semantic conversion has resolved names and full parent
    // paths, including ancestors outside a filtered/paged result. It is never model output.
    let mut result = crate::world_state::workflow_projection::semantic_state(result);
    if let Some(fields) = result.as_object_mut() {
        fields.remove("_directory");
        fields.remove("background");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn raw_runtime() -> Value {
        json!({"available":true,"observedAt":123,"instanceId":"org-id","currentNodeId":"a",
            "scope":{"departmentCount":2},"_directory":{"departments":[
                {"id":"parent","name":"Research","parentId":null},
                {"id":"child","name":"Applied","parentId":"parent"}]},
            "runtime":{"nodes":[
                {"nodeId":"a","nodeName":"Alice","departmentId":"child","state":"waiting_approval",
                    "paused":false,"waitingForApproval":true,"waitingForInteraction":false,
                    "pendingCount":2,"processingCount":1,"latestRun":{"status":"in_progress"}},
                {"nodeId":"b","nodeName":"Bob","departmentId":"parent","state":"stopped",
                    "paused":true,"pendingCount":3,"processingCount":0,"latestRun":{"status":"cancelled"}},
                {"nodeId":"c","nodeName":"Cara","departmentId":null,"state":"idle",
                    "pendingCount":0,"processingCount":0,"latestRun":{"status":"failed"}}]}})
    }
    #[test]
    fn organization_query_filters_enriched_status_before_paging_and_preserves_complete_runtime() {
        let query = StateQuery {
            view: StateView::Runtime,
            status: Some("waiting_approval".into()),
            limit: 1,
            ..Default::default()
        };
        let result = state_for_model(raw_runtime(), &query);
        assert_eq!(result["page"]["total"], 1);
        assert_eq!(result["page"]["returned"], 1);
        assert_eq!(result["runtime"]["members"][0]["member"], "Alice");
        assert_eq!(
            result["runtime"]["members"][0]["department"],
            "Research/Applied"
        );
        assert_eq!(result["runtime"]["members"][0]["state"], "waiting_approval");
        assert_eq!(result["runtime"]["members"][0]["waitingForApproval"], true);
        assert_eq!(result["runtime"]["members"][0]["processingCount"], 1);
        assert!(result.get("_directory").is_none());
        assert!(!result.to_string().contains("org-id"));
        let empty = state_for_model(
            raw_runtime(),
            &StateQuery {
                status: Some("compacting".into()),
                ..query
            },
        );
        assert_eq!(empty["page"]["total"], 0);
        assert_eq!(empty["page"]["empty"], true);
        assert_eq!(empty["runtime"]["members"], json!([]));
    }
    #[test]
    fn organization_overview_aggregates_full_candidates_after_host_attention_without_a_directory() {
        let result = state_for_model(
            raw_runtime(),
            &StateQuery {
                limit: 1,
                ..Default::default()
            },
        );
        assert_eq!(result["overview"]["memberCount"], 3);
        assert_eq!(result["overview"]["departmentCount"], 2);
        assert_eq!(result["overview"]["stateCounts"]["waiting_approval"], 1);
        assert_eq!(result["overview"]["stateCounts"]["running"], 0);
        assert_eq!(result["overview"]["mailBacklog"]["pendingCount"], 5);
        assert_eq!(result["overview"]["attention"]["total"], 2);
        assert_eq!(result["overview"]["attention"]["truncated"], true);
        assert!(result.get("runtime").is_none());
        assert!(result.get("members").is_none());
    }
    #[test]
    fn organization_query_pages_and_empty_pages_have_explicit_scope() {
        let query = StateQuery {
            view: StateView::Runtime,
            limit: 2,
            ..Default::default()
        };
        let first = state_for_model(raw_runtime(), &query);
        assert_eq!(first["observedAt"], 123);
        assert_eq!(first["page"]["total"], 3);
        assert_eq!(first["page"]["nextCursor"], 2);
        let next = state_for_model(
            raw_runtime(),
            &StateQuery {
                cursor: Some(2),
                ..query.clone()
            },
        );
        assert_eq!(next["page"]["returned"], 1);
        assert!(next["page"]["nextCursor"].is_null());
        let empty = state_for_model(
            raw_runtime(),
            &StateQuery {
                cursor: Some(99),
                ..query
            },
        );
        assert_eq!(empty["page"]["returned"], 0);
        assert_eq!(empty["page"]["total"], 3);
    }
    #[test]
    fn organization_query_rejects_unrelated_filters_and_removed_all_view() {
        for value in [
            json!({"view":"all"}),
            json!({"limit":0}),
            json!({"limit":51}),
        ] {
            assert!(serde_json::from_value::<StateQuery>(value).is_err());
        }
        for value in [
            json!({"status":"running"}),
            json!({"includeMail":true}),
            json!({"view":"runtime","includeMail":true}),
            json!({"view":"structure","nodeId":"a"}),
            json!({"view":"members","status":"idle"}),
            json!({"view":"runtime","status":"invalid"}),
            json!({"view":"members","search":"   "}),
            json!({"cursor":0}),
        ] {
            assert!(
                serde_json::from_value::<StateQuery>(value.clone())
                    .unwrap()
                    .validate()
                    .is_err(),
                "{value}"
            );
        }
        assert!(StateQuery {
            view: StateView::Runtime,
            node_id: Some("a".into()),
            include_mail: true,
            ..Default::default()
        }
        .validate()
        .is_ok());
        assert!(serde_json::from_value::<MailboxQuery>(json!({"status":"applied"})).is_err());
        assert_eq!(
            serde_json::from_value::<StateQuery>(json!({}))
                .unwrap()
                .view,
            StateView::Overview
        );
    }
}
