use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Only the model boundary uses readable names. Authoritative snapshots, renderer projections,
/// journal identities and permission checks retain their stable IDs.
pub(crate) fn for_model(section_id: &str, mut projection: Value) -> Value {
    if !matches!(
        section_id,
        "organization.execution" | "organization.awareness"
    ) {
        return projection;
    }
    if section_id == "organization.awareness" {
        if let Some(fields) = projection.as_object_mut() {
            // Mail arrivals have their own small section; outgoing history belongs to the mailbox.
            fields.remove("recentSent");
            fields.remove("mailbox");
            fields.remove("currentInputCount");
        }
    }
    semantic_state(projection)
}

#[derive(Default)]
struct Directory {
    members: BTreeMap<String, String>,
    departments: BTreeMap<String, (String, Option<String>)>,
    models: BTreeMap<String, String>,
}

impl Directory {
    fn collect(&mut self, value: &Value) {
        match value {
            Value::Array(values) => values.iter().for_each(|value| self.collect(value)),
            Value::Object(fields) => {
                if let (Some(id), Some(name)) = (
                    fields.get("nodeId").and_then(Value::as_str),
                    fields.get("nodeName").and_then(Value::as_str),
                ) {
                    self.members.insert(id.into(), name.into());
                }
                if let Some(departments) = fields.get("departments").and_then(Value::as_array) {
                    for department in departments {
                        if let (Some(id), Some(name)) =
                            (department["id"].as_str(), department["name"].as_str())
                        {
                            self.departments.insert(
                                id.into(),
                                (
                                    name.into(),
                                    department["parentId"].as_str().map(String::from),
                                ),
                            );
                        }
                    }
                }
                if let Some(models) = fields.get("availableModels").and_then(Value::as_array) {
                    for model in models {
                        if let (Some(id), Some(name)) =
                            (model["modelConfigId"].as_str(), model["name"].as_str())
                        {
                            self.models.insert(id.into(), name.into());
                        }
                    }
                }
                fields.values().for_each(|value| self.collect(value));
            }
            _ => {}
        }
    }

    fn department(&self, id: &str) -> Option<String> {
        let mut current = Some(id);
        let mut visited = BTreeSet::new();
        let mut names = Vec::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return None;
            }
            let (name, parent) = self.departments.get(id)?;
            names.push(name.trim());
            current = parent.as_deref();
        }
        names.reverse();
        Some(names.join("/"))
    }
}

fn rename(fields: &mut Map<String, Value>, from: &str, to: &str) {
    if let Some(value) = fields.remove(from) {
        fields.entry(to).or_insert(value);
    }
}

/// Shared model-only projection for state reads and the automatic organization sections. Do not
/// apply it to storage or renderer results. String values (tasks and mail bodies) stay opaque.
pub(crate) fn semantic_state(mut value: Value) -> Value {
    let mut directory = Directory::default();
    directory.collect(&value);
    semantic_fields(&mut value, &directory);
    value
}

fn semantic_fields(value: &mut Value, directory: &Directory) {
    let fields = match value {
        Value::Array(values) => {
            values
                .iter_mut()
                .for_each(|value| semantic_fields(value, directory));
            return;
        }
        Value::Object(fields) => fields,
        _ => return,
    };
    if let Some(departments) = fields.get_mut("departments").and_then(Value::as_array_mut) {
        for department in departments {
            let Some(fields) = department.as_object_mut() else {
                continue;
            };
            if let Some(path) = fields
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| directory.department(id))
            {
                fields.insert("department".into(), Value::String(path));
                fields.remove("name");
            }
            if let Some(parent) = fields.remove("parentId") {
                let path = parent.as_str().and_then(|id| directory.department(id));
                fields.insert(
                    "parentDepartment".into(),
                    path.map(Value::String).unwrap_or(Value::Null),
                );
            }
            for key in ["x", "y", "width", "height"] {
                fields.remove(key);
            }
        }
    }
    if let Some(id) = fields.remove("currentNodeId") {
        if let Some(name) = id.as_str().and_then(|id| directory.members.get(id)) {
            fields.insert("currentMember".into(), Value::String(name.clone()));
        }
    }
    for (from, to) in [
        ("departmentId", "department"),
        ("scopeDepartmentId", "department"),
    ] {
        if let Some(id) = fields.remove(from) {
            let path = id.as_str().and_then(|id| directory.department(id));
            fields.insert(to.into(), path.map(Value::String).unwrap_or(Value::Null));
        }
    }
    if let Some(models) = fields
        .get_mut("availableModels")
        .and_then(Value::as_array_mut)
    {
        *models = models
            .iter()
            .filter_map(|model| {
                model
                    .as_str()
                    .or_else(|| model["name"].as_str())
                    .map(|name| Value::String(name.into()))
            })
            .collect();
    }
    if let Some(id) = fields.remove("modelConfigId") {
        let name = id.as_str().and_then(|id| directory.models.get(id));
        if id.is_string() && name.is_none() {
            fields.insert("modelUnavailable".into(), Value::Bool(true));
        }
        fields.insert(
            "model".into(),
            name.cloned().map(Value::String).unwrap_or(Value::Null),
        );
    }
    if fields.contains_key("pendingCount") {
        fields.remove("queuedInputCount");
    }
    if fields.contains_key("processingCount") {
        fields.remove("currentInputCount");
    }
    for key in [
        "id",
        "instanceId",
        "templateId",
        "templateRevision",
        "executionVersion",
        "organizationRevision",
        "membershipVersion",
        "nodeId",
        "conversationId",
        "sourceNodeId",
        "targetNodeId",
        "sourceConversationId",
        "targetConversationId",
        "sourceConversationTitle",
        "targetConversationTitle",
        "runId",
        "activeRunId",
        "inputId",
        "inputIds",
        "currentInputIds",
        "deliveryId",
        "sequence",
        "kind",
        "handlingRule",
        "queryHint",
    ] {
        fields.remove(key);
    }
    for (from, to) in [
        ("nodeName", "member"),
        ("nodes", "members"),
        ("totalNodeCount", "memberCount"),
        ("nodesTruncated", "membersTruncated"),
        ("sourceNodeName", "from"),
        ("targetNodeName", "to"),
        ("replyToMessageId", "replyTo"),
        ("workflowName", "organizationName"),
        ("inputs", "mail"),
        ("inputsTruncated", "mailTruncated"),
    ] {
        rename(fields, from, to);
    }
    if fields.contains_key("detailsQueryHint") {
        let hint = if fields.contains_key("summaryPolicy") {
            "Unchanged overview fields are omitted. Query a member by name for its full current state."
        } else {
            "Query this member by name for full responsibilities."
        };
        fields.insert("detailsQueryHint".into(), Value::String(hint.into()));
    }
    if fields.contains_key("configurationRule") {
        fields.insert("configurationRule".into(), Value::String(
            "memberDefaults are saved settings; nextTurn is the next automatic wake's settings. Editing does not change an active turn. Choose model names from availableModels.".into(),
        ));
    }
    fields
        .values_mut()
        .for_each(|value| semantic_fields(value, directory));
}

pub(crate) fn remove_duplicate_mail_counts(node: &mut Value) {
    let Some(fields) = node.as_object_mut() else {
        return;
    };
    if fields.contains_key("pendingCount") {
        fields.remove("queuedInputCount");
    }
    if fields.contains_key("processingCount") {
        fields.remove("currentInputCount");
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use serde_json::json;

    fn stored_snapshot(sequence: u64, version: u64, task: &str) -> WorldStateSnapshot {
        let execution = json!({"available":true,"organization":{
            "instanceId":"team","name":"Mail team","templateId":format!("template-{version}"),
            "templateRevision":version,"executionVersion":format!("execution-{version}"),
            "nodeId":"reviewer","nodeName":"Reviewer","background":"Shared background",
            "receives":"Changes","task":task,"delivers":"Review results",
            "members":[{"nodeId":"writer","nodeName":"Writer","task":"Write"}],"enabled":true
        }});
        let awareness = json!({"available":true,"instanceId":"team","executionVersion":format!("execution-{version}"),
            "currentNodeId":"reviewer","nodes":[{"nodeId":"writer","nodeName":"Writer","state":"running"}]});
        // Intentionally retain the original unfiltered projections: these represent an existing
        // persisted journal, so tests cover replay rather than only new extension publications.
        WorldStateSnapshot::new(
            "epoch",
            sequence,
            [
                ("organization.execution", execution),
                ("organization.awareness", awareness),
            ]
            .into_iter()
            .map(|(id, value)| {
                WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension(id).unwrap(),
                    WorldStateLifetime::Conversation,
                    value.clone(),
                    value,
                )
                .unwrap()
            })
            .collect(),
        )
        .unwrap()
    }

    fn assert_no_template_metadata(text: &str) {
        for field in [
            "templateId",
            "templateRevision",
            "executionVersion",
            "template-1",
            "execution-1",
        ] {
            assert!(!text.contains(field), "model projection exposed {field}");
        }
    }

    #[test]
    fn organization_projection_hides_metadata_and_preserves_member_authored_content() {
        let raw = json!({"available":true,"organization":{"name":"Workflow research",
            "templateId":"private", "templateRevision":1,"executionVersion":"private-v1",
            "task":"Review workflow examples"}});
        let projected = super::for_model("organization.execution", raw.clone());
        assert_eq!(projected["organization"]["name"], "Workflow research");
        assert_eq!(
            projected["organization"]["task"],
            "Review workflow examples"
        );
        assert_no_template_metadata(&projected.to_string());
        assert_eq!(raw["organization"]["templateId"], "private");
        let projected = super::for_model(
            "organization.awareness",
            json!({"executionVersion":"private","nodes":[]}),
        );
        assert!(projected.get("executionVersion").is_none());
    }

    #[test]
    fn workflow_replay_and_rebase_hide_template_metadata_without_changing_host_state() {
        let stored = stored_snapshot(0, 1, "Review changes");
        let canonical = stored.canonical_json();
        let restored: WorldStateSnapshot = serde_json::from_str(&canonical).unwrap();
        for snapshot in [
            restored.clone(),
            restored.rebase("compacted-epoch").unwrap(),
        ] {
            let text = snapshot
                .model_projection(WorldStateLifetime::Conversation)
                .unwrap()
                .render_sanitized_text();
            assert_no_template_metadata(&text);
            for useful in [
                "Mail team",
                "Shared background",
                "Changes",
                "Review changes",
                "Review results",
                "Writer",
                "running",
            ] {
                assert!(text.contains(useful), "working context lost {useful}");
            }
            assert!(snapshot.canonical_json().contains("templateId"));
            snapshot.validate().unwrap();
        }
        assert_eq!(stored.canonical_json(), canonical);
    }

    #[test]
    fn workflow_internal_version_changes_do_not_create_model_diffs() {
        let before = stored_snapshot(0, 1, "Review changes");
        let after = stored_snapshot(1, 2, "Review changes");
        let diff = WorldStateDiff::between(&before, &after).unwrap();
        assert_ne!(before.revision, after.revision);
        assert_eq!(
            before
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap(),
            after
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap()
        );
        assert_eq!(
            diff.model_projection_against(&before, WorldStateLifetime::Conversation)
                .unwrap(),
            None
        );
        let changed_task = stored_snapshot(2, 3, "Review final draft");
        let diff = WorldStateDiff::between(&after, &changed_task).unwrap();
        let text = diff
            .model_projection_against(&after, WorldStateLifetime::Conversation)
            .unwrap()
            .unwrap()
            .render_sanitized_text();
        assert_no_template_metadata(&text);
        assert!(text.contains("Review final draft"));
        assert!(!text.contains("execution-3"));
        assert!(!text.contains("template-3"));
    }
    #[test]
    fn organization_hierarchy_authority_changes_survive_diff_and_replay_without_canvas_geometry() {
        let snapshot = |sequence, role: &str, rank| {
            let state = json!({"available":true,"organization":{"instanceId":"team","organizationRevision":sequence+1,
                "executionVersion":"stable-member","managementRole":role,"rank":rank,"departmentId":"engineering",
                "departments":[{"id":"engineering","name":"Engineering","parentId":null,"x":1234,"y":5678,"width":400,"height":300}],
                "members":[{"nodeId":"reviewer","rank":20,"managementRole":"member","departmentId":"engineering"}]}});
            WorldStateSnapshot::new(
                "authority",
                sequence,
                vec![WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension("organization.execution").unwrap(),
                    WorldStateLifetime::Conversation,
                    state.clone(),
                    state,
                )
                .unwrap()],
            )
            .unwrap()
        };
        let before = snapshot(0, "organization_admin", 90);
        let after = snapshot(1, "department_admin", 80);
        let diff = WorldStateDiff::between(&before, &after).unwrap();
        let projection = diff
            .model_projection_against(&before, WorldStateLifetime::Conversation)
            .unwrap()
            .unwrap()
            .render_sanitized_text();
        assert!(projection.contains("department_admin"));
        assert!(!projection.contains("organizationRevision"));
        assert!(projection.contains("Engineering"));
        assert!(!projection.contains("1234"));
        assert!(!projection.contains("stable-member"));
        let replay = WorldStateReducer::fold(before, &[diff]).unwrap();
        assert_eq!(replay.revision, after.revision);
        assert!(replay.canonical_json().contains("1234"));
    }

    #[test]
    fn presentation_only_organization_revisions_do_not_repeat_roles_or_background() {
        let snapshot = |sequence, x, task: &str| {
            let value = json!({"available":true,"organization":{"instanceId":"team","organizationRevision":sequence+1,
                "nodeId":"a","background":"Long shared background","task":task,
                "departments":[{"id":"d","name":"Department","parentId":null,"x":x,"y":0,"width":400,"height":300}],
                "members":[{"nodeId":"b","task":"Write"}]}});
            WorldStateSnapshot::new(
                "semantic",
                sequence,
                vec![WorldStateSectionEnvelope::model_visible(
                    WorldStateSectionId::extension("organization.execution").unwrap(),
                    WorldStateLifetime::Conversation,
                    value.clone(),
                    value,
                )
                .unwrap()],
            )
            .unwrap()
        };
        let before = snapshot(0, 0, "Review");
        let moved = snapshot(1, 200, "Review");
        assert_ne!(before.revision, moved.revision);
        assert!(WorldStateDiff::between(&before, &moved)
            .unwrap()
            .model_projection_against(&before, WorldStateLifetime::Conversation)
            .unwrap()
            .is_none());
        let edited = snapshot(2, 200, "Review final copy");
        let change = WorldStateDiff::between(&moved, &edited)
            .unwrap()
            .model_projection_against(&moved, WorldStateLifetime::Conversation)
            .unwrap()
            .unwrap()
            .render_sanitized_text();
        assert!(change.contains("Review final copy"));
        assert!(!change.contains("organizationRevision"));
    }

    #[test]
    fn organization_mail_summary_uses_one_count_per_lifecycle_state() {
        let projected = super::for_model(
            "organization.awareness",
            json!({"currentInputCount":2,"nodes":[{
            "pendingCount":3,"queuedInputCount":3,"processingCount":2,"currentInputCount":2,"currentInputIds":["one","two"]}]}),
        );
        assert!(projected.get("currentInputCount").is_none());
        assert!(projected["members"][0].get("currentInputCount").is_none());
        assert!(projected["members"][0].get("queuedInputCount").is_none());
        assert_eq!(projected["members"][0]["pendingCount"], 3);
        assert_eq!(projected["members"][0]["processingCount"], 2);
        assert!(projected["members"][0].get("currentInputIds").is_none());
    }
    #[test]
    fn readable_directory_survives_replay_without_routing_ids_or_duplicate_mail_summaries() {
        let raw = json!({"available":true,"organization":{
            "instanceId":"internal-org","nodeId":"internal-boss","nodeName":"Boss",
            "departmentId":"internal-payroll","managementRole":"department_admin",
            "departments":[
                {"id":"internal-hr","name":"人事部","parentId":null,"x":100,"y":200},
                {"id":"internal-payroll","name":"薪酬组","parentId":"internal-hr"}],
            "members":[{"nodeId":"internal-peer","nodeName":"周宁","conversationId":"internal-chat",
                "membershipVersion":"internal-incarnation","departmentId":"internal-payroll",
                "task":"Read the literal word nodeId in documents"}]},
            "management":{"scopeDepartmentId":"internal-payroll"}});
        let projected = super::for_model("organization.execution", raw.clone());
        assert_eq!(projected["organization"]["member"], "Boss");
        assert_eq!(projected["organization"]["members"][0]["member"], "周宁");
        assert_eq!(
            projected["organization"]["members"][0]["department"],
            "人事部/薪酬组"
        );
        assert_eq!(projected["management"]["department"], "人事部/薪酬组");
        assert_eq!(
            projected["organization"]["departments"][1]["parentDepartment"],
            "人事部"
        );
        assert!(!projected.to_string().contains("internal-"));
        assert_eq!(
            projected["organization"]["members"][0]["task"],
            raw["organization"]["members"][0]["task"]
        );
        assert_eq!(
            super::for_model("organization.execution", projected.clone()),
            projected
        );
        assert!(raw.to_string().contains("internal-incarnation"));

        let awareness = super::for_model(
            "organization.awareness",
            json!({
            "currentNodeId":"internal-boss","nodes":[{"nodeId":"internal-boss","nodeName":"Boss","activeRunId":"internal-run","state":"running","pendingCount":1}],
            "recentSent":[{"messageId":"m","targetNodeId":"internal-peer"}],
            "mailbox":{"recentArrivals":[{"sourceNodeId":"internal-peer"}]}}),
        );
        assert_eq!(awareness["currentMember"], "Boss");
        assert_eq!(awareness["members"][0]["state"], "running");
        assert!(!awareness.to_string().contains("internal-"));
        assert!(awareness.get("recentSent").is_none());
        assert!(awareness.get("mailbox").is_none());
    }

    #[test]
    fn configuration_uses_only_available_readable_model_choices() {
        let raw = json!({"configuration":{
            "members":[{"nodeId":"internal-member","nodeName":"周宁",
                "memberDefaults":{"modelConfigId":"internal-model","permissionMode":"default"},
                "nextTurn":{"modelConfigId":"missing-model","permissionMode":"custom"},
                "activeRun":{"runId":"internal-run","configuration":"frozen_for_this_turn_not_reported_here"}}],
            "availableModels":[{"modelConfigId":"internal-model","name":"Writing assistant"}],
            "callerCurrentRun":{"modelConfigId":"internal-model","allowedPermissionModes":["default"]},
            "configurationRule":"Use nextTurn.modelConfigId"}});
        let projected = super::semantic_state(raw.clone());
        assert_eq!(
            projected["configuration"]["availableModels"],
            json!(["Writing assistant"])
        );
        assert_eq!(
            projected["configuration"]["members"][0]["memberDefaults"]["model"],
            "Writing assistant"
        );
        assert!(projected["configuration"]["members"][0]["nextTurn"]["model"].is_null());
        assert_eq!(
            projected["configuration"]["callerCurrentRun"]["model"],
            "Writing assistant"
        );
        assert_eq!(super::semantic_state(projected.clone()), projected);
        assert!(!projected.to_string().contains("internal-"));
        assert!(!projected.to_string().contains("modelConfigId"));
        assert!(raw.to_string().contains("internal-model"));
    }
}
