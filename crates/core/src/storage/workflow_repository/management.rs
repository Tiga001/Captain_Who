use super::*;
use crate::workflow::{NodeConfig, WorkflowPermissionMode};
use crate::workflow_management::{
    Binding, EditingDraft, Instance, InvalidEditingDraft, Request as ManagementRequest, Usage,
    UsageInstance,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

pub(super) fn list_instances(c: &Connection) -> Result<Vec<Instance>, Error> {
    let mut statement = c.prepare("SELECT instance_id, template_id, template_revision, name, color, revision, updated_at, needs_review, (enabled AND (running OR EXISTS(SELECT 1 FROM workflow_instance_bindings b JOIN conversation_turn_traces t ON (t.conversation_id=b.conversation_id OR t.conversation_id IN (SELECT conversation_id FROM agent_nodes WHERE root_conversation_id=b.conversation_id)) WHERE b.instance_id=workflow_instances.instance_id AND t.terminal_status='in_progress'))), enabled, project_id FROM workflow_instances ORDER BY updated_at DESC, instance_id").map_err(storage_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok(Instance {
                id: r.get(0)?,
                template_id: r.get(1)?,
                template_revision: r.get(2)?,
                name: r.get(3)?,
                color: r.get(4)?,
                revision: r.get(5)?,
                updated_at: r.get(6)?,
                needs_review: r.get(7)?,
                running: r.get(8)?,
                enabled: r.get(9)?,
                project_id: r.get(10)?,
                bindings: vec![],
            })
        })
        .map_err(storage_error)?;
    let mut result: Vec<Instance> = rows
        .collect::<rusqlite::Result<_>>()
        .map_err(storage_error)?;
    for instance in &mut result {
        let mut query = c.prepare("SELECT node_id, conversation_id FROM workflow_instance_bindings WHERE instance_id=?1 ORDER BY node_id").map_err(storage_error)?;
        instance.bindings = query
            .query_map([&instance.id], |r| {
                Ok(Binding {
                    node_id: r.get(0)?,
                    conversation_id: r.get(1)?,
                })
            })
            .map_err(storage_error)?
            .collect::<rusqlite::Result<_>>()
            .map_err(storage_error)?;
    }
    Ok(result)
}

pub(super) fn list_usages(c: &Connection) -> Result<Vec<Usage>, Error> {
    let mut grouped: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
    for instance in list_instances(c)? {
        grouped
            .entry(instance.template_id.clone())
            .or_default()
            .push(instance);
    }
    grouped.into_iter().map(|(template_id, mut instances)| {
        instances.sort_by(|a,b|a.id.cmp(&b.id));
        let mut digest = Sha256::new();
        let mut usage = vec![];
        for instance in instances {
            let mut statement = c.prepare("SELECT DISTINCT p.name FROM workflow_instance_bindings b JOIN conversations c ON c.id=b.conversation_id JOIN projects p ON p.id=c.project_id WHERE b.instance_id=?1 ORDER BY p.name").map_err(storage_error)?;
            let project_names: Vec<String> = statement.query_map([&instance.id],|r|r.get(0)).map_err(storage_error)?.collect::<rusqlite::Result<_>>().map_err(storage_error)?;
            digest.update(serde_json::to_vec(&(&instance, &project_names)).map_err(storage_error)?);
            usage.push(UsageInstance {id:instance.id,name:instance.name,running:instance.running,project_names});
        }
        Ok(Usage {template_id,usage_revision:format!("{:x}",digest.finalize()),instances:usage})
    }).collect()
}

pub(super) fn list_drafts(
    c: &Connection,
    models: &HashSet<String>,
) -> Result<(Vec<EditingDraft>, Vec<InvalidEditingDraft>), Error> {
    let mut statement = c.prepare("SELECT template_id, definition_json, base_revision, revision, updated_at FROM workflow_editing_drafts ORDER BY updated_at DESC, template_id").map_err(storage_error)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .map_err(storage_error)?;
    let mut drafts = vec![];
    let mut invalid_drafts = vec![];
    for row in rows {
        let (id, json, base_revision, revision, updated_at) = row.map_err(storage_error)?;
        if !(1..=MAX_SAFE_REVISION).contains(&base_revision)
            || !(1..=MAX_SAFE_REVISION).contains(&revision)
            || updated_at < 0
        {
            return Err(Error::Storage(
                "Invalid persisted workflow draft metadata".into(),
            ));
        }
        match parse_persisted_definition(&json, &id, models) {
            Ok((definition, _)) => drafts.push(EditingDraft {
                definition,
                base_revision: base_revision as u64,
                revision: revision as u64,
                updated_at,
            }),
            Err(reason) => invalid_drafts.push(InvalidEditingDraft {
                name: recovery_display_name(&json, &id),
                id,
                base_revision: base_revision as u64,
                revision: revision as u64,
                updated_at,
                reason,
            }),
        }
    }
    Ok((drafts, invalid_drafts))
}

pub(super) fn template_in_use(c: &Connection, id: &str) -> Result<bool, Error> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE template_id=?1)",
        [id],
        |r| r.get(0),
    )
    .map_err(storage_error)
}

pub(super) fn publish(
    c: &Connection,
    definition: &Definition,
    expected_revision: u64,
    expected_usage_revision: Option<&str>,
    expected_draft_revision: Option<u64>,
    models: &HashSet<String>,
) -> Result<(), Error> {
    let issues = definition.validate(models).map_err(Error::Invalid)?;
    if let Some(usage) = list_usages(c)?
        .into_iter()
        .find(|usage| usage.template_id == definition.id)
    {
        if usage.instances.iter().any(|instance| instance.running) {
            return Err(Error::Conflict("workflow_template_running".into()));
        }
        if expected_usage_revision != Some(usage.usage_revision.as_str()) {
            return Err(Error::Conflict("workflow_usage_changed".into()));
        }
    }
    let current_draft: Option<u64> = c
        .query_row(
            "SELECT revision FROM workflow_editing_drafts WHERE template_id=?1",
            [&definition.id],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    if current_draft.unwrap_or(0) != expected_draft_revision.unwrap_or(0) {
        return Err(Error::Conflict("workflow_draft_changed".into()));
    }
    save(c, definition, expected_revision, issues.is_empty())?;
    let agent_ids: HashSet<_> = definition
        .nodes
        .iter()
        .filter(|node| matches!(node.config, NodeConfig::Agent(_)))
        .map(|node| node.id.as_str())
        .collect();
    for instance in list_instances(c)?
        .into_iter()
        .filter(|instance| instance.template_id == definition.id)
    {
        crate::storage::workflow_execution_repository::invalidate_instance(
            c,
            &instance.id,
            "Workflow template was updated",
        )
        .map_err(Error::Storage)?;
        for binding in instance.bindings {
            if !agent_ids.contains(binding.node_id.as_str()) {
                c.execute(
                    "DELETE FROM workflow_instance_bindings WHERE instance_id=?1 AND node_id=?2",
                    params![instance.id, binding.node_id],
                )
                .map_err(storage_error)?;
            }
        }
        c.execute("UPDATE workflow_instances SET enabled=0, needs_review=1, revision=revision+1, updated_at=MAX(updated_at+1,?1), last_request_json='{}' WHERE instance_id=?2",params![now_ms(),instance.id]).map_err(storage_error)?;
    }
    c.execute(
        "DELETE FROM workflow_editing_drafts WHERE template_id=?1",
        [&definition.id],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn template(c: &Connection, id: &str, expected: u64) -> Result<Definition, Error> {
    validate_id(id)?;
    revision_to_sql(expected)?;
    let row: Option<(String, u64)> = c
        .query_row(
            "SELECT definition_json,revision FROM workflow_definitions WHERE workflow_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage_error)?;
    let Some((json, revision)) = row else {
        return Err(revision_conflict());
    };
    if revision != expected {
        return Err(revision_conflict());
    }
    serde_json::from_str(&json).map_err(|_| Error::Invalid("workflow_template_unavailable".into()))
}

pub(super) fn request(
    c: &Connection,
    request: ManagementRequest,
    models: &HashSet<String>,
) -> Result<Vec<String>, Error> {
    let request_json = serde_json::to_string(&request).map_err(storage_error)?;
    match request {
        ManagementRequest::ListInstances {} => Ok(vec![]),
        ManagementRequest::SetInstanceEnabled {
            id,
            enabled,
            expected_revision,
        } => {
            validate_id(&id)?;
            let expected = revision_to_sql(expected_revision)?;
            let current = list_instances(c)?
                .into_iter()
                .find(|instance| instance.id == id)
                .ok_or_else(revision_conflict)?;
            if current.revision == expected_revision + 1 {
                let last: String = c
                    .query_row(
                        "SELECT last_request_json FROM workflow_instances WHERE instance_id=?1",
                        [&id],
                        |row| row.get(0),
                    )
                    .map_err(storage_error)?;
                let receipt: serde_json::Value =
                    serde_json::from_str(&last).map_err(storage_error)?;
                if receipt.get("request").and_then(|value| value.as_str())
                    == Some(request_json.as_str())
                {
                    return Ok(vec![]);
                }
            }
            if current.revision != expected_revision {
                return Err(revision_conflict());
            }
            if enabled {
                if current.needs_review {
                    return Err(Error::Invalid("workflow_instance_needs_review".into()));
                }
                let definition = template(c, &current.template_id, current.template_revision)?;
                if !definition
                    .validate(models)
                    .map_err(Error::Invalid)?
                    .is_empty()
                {
                    return Err(Error::Invalid("workflow_template_unavailable".into()));
                }
                let agents: HashSet<_> = definition
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.config, NodeConfig::Agent(_)))
                    .map(|node| node.id.as_str())
                    .collect();
                if current.bindings.len() != agents.len()
                    || current
                        .bindings
                        .iter()
                        .any(|binding| !agents.contains(binding.node_id.as_str()))
                {
                    return Err(Error::Invalid("workflow_bindings_incomplete".into()));
                }
                let unavailable: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM workflow_instance_bindings b LEFT JOIN conversations c ON c.id=b.conversation_id WHERE b.instance_id=?1 AND (c.id IS NULL OR c.archived_at IS NOT NULL OR EXISTS(SELECT 1 FROM agent_nodes a WHERE a.conversation_id=c.id AND a.parent_agent_id IS NOT NULL)))",[&id],|row|row.get(0)).map_err(storage_error)?;
                if unavailable {
                    return Err(Error::Invalid("workflow_bindings_incomplete".into()));
                }
                let occupied: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE instance_id<>?1 AND enabled=1 AND color=?2 COLLATE NOCASE)",params![id,current.color],|row|row.get(0)).map_err(storage_error)?;
                if occupied {
                    return Err(Error::Conflict("workflow_color_in_use".into()));
                }
            }
            if current.enabled == enabled {
                return Ok(vec![]);
            }
            c.execute("UPDATE workflow_instances SET enabled=?1,revision=?2,updated_at=MAX(updated_at+1,?3),last_request_json=?4 WHERE instance_id=?5",params![enabled,next_revision(expected)?,now_ms(),serde_json::json!({"request":request_json,"affected":[]}).to_string(),id]).map_err(storage_error)?;
            Ok(vec![])
        }
        ManagementRequest::Duplicate {
            id,
            expected_revision,
            new_id,
            name,
        } => {
            let mut definition = template(c, &id, expected_revision)?;
            definition.id = new_id;
            definition.name = name;
            let issues = definition.validate(models).map_err(Error::Invalid)?;
            save(c, &definition, 0, issues.is_empty())?;
            Ok(vec![])
        }
        ManagementRequest::SaveDraft {
            definition,
            expected_revision,
            expected_draft_revision,
        } => {
            template(c, &definition.id, expected_revision)?;
            definition.validate(models).map_err(Error::Invalid)?;
            let current: Option<u64> = c
                .query_row(
                    "SELECT revision FROM workflow_editing_drafts WHERE template_id=?1",
                    [&definition.id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage_error)?;
            if current.unwrap_or(0) != expected_draft_revision {
                return Err(revision_conflict());
            }
            let next = next_revision(revision_to_sql(expected_draft_revision)?)?;
            c.execute("INSERT INTO workflow_editing_drafts(template_id,definition_json,base_revision,revision,updated_at) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(template_id) DO UPDATE SET definition_json=excluded.definition_json,base_revision=excluded.base_revision,revision=excluded.revision,updated_at=MAX(workflow_editing_drafts.updated_at+1,excluded.updated_at)",params![definition.id,serde_json::to_string(&definition).map_err(storage_error)?,expected_revision,next,now_ms()]).map_err(storage_error)?;
            Ok(vec![])
        }
        ManagementRequest::DeleteDraft {
            id,
            expected_draft_revision,
        } => {
            let revision = revision_to_sql(expected_draft_revision)?;
            let changed = c
                .execute(
                    "DELETE FROM workflow_editing_drafts WHERE template_id=?1 AND revision=?2",
                    params![id, revision],
                )
                .map_err(storage_error)?;
            if changed != 1 {
                return Err(revision_conflict());
            }
            Ok(vec![])
        }
        ManagementRequest::DeleteInstance {
            id,
            expected_revision,
        } => {
            let revision = revision_to_sql(expected_revision)?;
            let running = list_instances(c)?
                .into_iter()
                .find(|instance| instance.id == id && instance.revision == revision as u64)
                .map(|instance| instance.running);
            if running == Some(true) {
                return Err(Error::Conflict("workflow_instance_running".into()));
            }
            if running.is_none() {
                return Err(revision_conflict());
            }
            crate::storage::workflow_execution_repository::invalidate_instance(
                c,
                &id,
                "Workflow instance was deleted",
            )
            .map_err(Error::Storage)?;
            c.execute("DELETE FROM workflow_instances WHERE instance_id=?1", [id])
                .map_err(storage_error)?;
            Ok(vec![])
        }
        ManagementRequest::SaveInstance {
            id,
            template_id,
            name,
            color,
            project_id,
            bindings,
            expected_revision,
            expected_template_revision,
        } => {
            validate_id(&id)?;
            if name.trim().is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
                return Err(Error::Invalid("Invalid workflow instance name".into()));
            }
            if color.len() != 7
                || !color.starts_with('#')
                || !color[1..].bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(Error::Invalid("Invalid workflow color".into()));
            }
            let expected = revision_to_sql(expected_revision)?;
            let current = list_instances(c)?
                .into_iter()
                .find(|instance| instance.id == id);
            // Repeating the exact successful confirmation is safe after a lost RPC response.
            if current
                .as_ref()
                .is_some_and(|instance| instance.revision == expected_revision + 1)
            {
                let last: String = c
                    .query_row(
                        "SELECT last_request_json FROM workflow_instances WHERE instance_id=?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .map_err(storage_error)?;
                let stored: serde_json::Value =
                    serde_json::from_str(&last).map_err(storage_error)?;
                if stored.get("request").and_then(|v| v.as_str()) == Some(request_json.as_str()) {
                    return serde_json::from_value(stored["affected"].clone())
                        .map_err(storage_error);
                }
            }
            if current
                .as_ref()
                .map(|instance| instance.revision)
                .unwrap_or(0)
                != expected_revision
            {
                return Err(revision_conflict());
            }
            if current.as_ref().is_some_and(|instance| instance.running) {
                return Err(Error::Conflict("workflow_instance_running".into()));
            }
            if current
                .as_ref()
                .is_some_and(|instance| instance.template_id != template_id)
            {
                return Err(Error::Invalid(
                    "An instance cannot change its template".into(),
                ));
            }
            // A destination project only controls auto-created chats, never existing bindings.
            // Validate before creating any conversation; the transaction also serializes deletion.
            if let Some(project_id) = &project_id {
                validate_id(project_id)?;
                let exists: bool = c
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                        [project_id],
                        |row| row.get(0),
                    )
                    .map_err(storage_error)?;
                if !exists {
                    return Err(Error::Invalid("workflow_project_missing".into()));
                }
            }
            // The enclosing immediate transaction serializes competing confirmations. Check
            // every persisted instance before creating conversations or changing composer state.
            // Confirming a new or edited binding activates the instance.
            let enabled = true;
            let color_in_use: bool = c
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM workflow_instances WHERE instance_id<>?1 AND enabled=1 AND color=?2 COLLATE NOCASE)",
                    params![id, color],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            if color_in_use {
                return Err(Error::Conflict("workflow_color_in_use".into()));
            }
            let definition = template(c, &template_id, expected_template_revision)?;
            if !definition
                .validate(models)
                .map_err(Error::Invalid)?
                .is_empty()
            {
                return Err(Error::Invalid("workflow_template_unavailable".into()));
            }
            let agents: Vec<_> = definition
                .nodes
                .iter()
                .filter_map(|node| {
                    if let NodeConfig::Agent(config) = &node.config {
                        Some((node, config))
                    } else {
                        None
                    }
                })
                .collect();
            let mut inputs = HashMap::new();
            let mut conversations = HashSet::new();
            for binding in bindings {
                if !agents.iter().any(|(node, _)| node.id == binding.node_id)
                    || inputs.contains_key(&binding.node_id)
                {
                    return Err(Error::Invalid("Invalid workflow binding node".into()));
                }
                if let Some(conversation_id) = &binding.conversation_id {
                    validate_id(conversation_id)?;
                    if !conversations.insert(conversation_id.clone()) {
                        return Err(Error::Invalid("workflow_conversation_already_bound".into()));
                    }
                }
                inputs.insert(binding.node_id, binding.conversation_id);
            }
            if current.is_none() {
                let count: i64 = c
                    .query_row("SELECT COUNT(*) FROM workflow_instances", [], |r| r.get(0))
                    .map_err(storage_error)?;
                if count >= 1000 {
                    return Err(Error::Invalid(
                        "At most 1000 workflow instances can be saved".into(),
                    ));
                }
            }
            let next = next_revision(expected)?;
            let now = now_ms();
            let previous: HashMap<_, _> = current
                .as_ref()
                .map(|instance| {
                    instance
                        .bindings
                        .iter()
                        .map(|b| (b.node_id.clone(), b.conversation_id.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let mut resolved = vec![];
            let mut affected = vec![];
            for (node, config) in agents {
                let input = inputs.remove(&node.id).flatten();
                let conversation_id = if let Some(conversation_id) = input {
                    let row: Option<(Option<String>, Option<i64>)> = c
                        .query_row(
                            "SELECT model_id,archived_at FROM conversations WHERE id=?1",
                            [&conversation_id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()
                        .map_err(storage_error)?;
                    let Some((_model, archived)) = row else {
                        return Err(Error::Invalid("workflow_conversation_missing".into()));
                    };
                    if archived.is_some() {
                        return Err(Error::Invalid("workflow_conversation_archived".into()));
                    }
                    let child:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM agent_nodes WHERE conversation_id=?1 AND parent_agent_id IS NOT NULL)",[&conversation_id],|r|r.get(0)).map_err(storage_error)?;
                    if child {
                        return Err(Error::Invalid(
                            "Only independent conversations can be bound".into(),
                        ));
                    }
                    let owner:Option<String>=c.query_row("SELECT instance_id FROM workflow_instance_bindings WHERE conversation_id=?1",[&conversation_id],|r|r.get(0)).optional().map_err(storage_error)?;
                    if owner.as_ref().is_some_and(|owner| owner != &id) {
                        return Err(Error::Conflict(
                            "workflow_conversation_already_bound".into(),
                        ));
                    }
                    conversation_id
                } else {
                    let conversation_id = uuid::Uuid::new_v4().to_string();
                    c.execute("INSERT INTO conversations(id,project_id,model_id,title,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)",params![conversation_id,project_id,config.model_config_id,node.name,now]).map_err(storage_error)?;
                    conversation_id
                };
                if previous.get(&node.id) != Some(&conversation_id) {
                    let permission = match config.permission_mode {
                        WorkflowPermissionMode::Default => "default",
                        WorkflowPermissionMode::Custom => "custom",
                        WorkflowPermissionMode::Full => "full",
                    };
                    crate::storage::composer_draft_repository::set_composer_configuration(
                        c,
                        &conversation_id,
                        config.model_config_id.as_deref(),
                        permission,
                        now,
                    )
                    .map_err(storage_error)?;
                    affected.push(conversation_id.clone());
                }
                resolved.push(Binding {
                    node_id: node.id.clone(),
                    conversation_id,
                });
            }
            if current.is_some()
                && (resolved.len() != previous.len()
                    || resolved.iter().any(|binding| {
                        previous.get(&binding.node_id) != Some(&binding.conversation_id)
                    }))
            {
                crate::storage::workflow_execution_repository::invalidate_instance(
                    c,
                    &id,
                    "Workflow conversation bindings were changed",
                )
                .map_err(Error::Storage)?;
            }
            c.execute("INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,needs_review,running,last_request_json,enabled,project_id) VALUES (?1,?2,?3,?4,?5,?6,?7,0,0,?8,?9,?10) ON CONFLICT(instance_id) DO UPDATE SET template_revision=excluded.template_revision,name=excluded.name,color=excluded.color,enabled=excluded.enabled,project_id=excluded.project_id,revision=excluded.revision,updated_at=MAX(workflow_instances.updated_at+1,excluded.updated_at),needs_review=0,last_request_json=excluded.last_request_json",params![id,template_id,expected_template_revision,name,color,next,now,serde_json::json!({"request":request_json,"affected":affected}).to_string(),enabled,project_id]).map_err(storage_error)?;
            c.execute(
                "DELETE FROM workflow_instance_bindings WHERE instance_id=?1",
                [&id],
            )
            .map_err(storage_error)?;
            for binding in resolved {
                c.execute("INSERT INTO workflow_instance_bindings(instance_id,node_id,conversation_id) VALUES (?1,?2,?3)",params![id,binding.node_id,binding.conversation_id]).map_err(storage_error)?;
            }
            Ok(affected)
        }
    }
}
