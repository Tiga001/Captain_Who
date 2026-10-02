use super::error;
use crate::organization_personnel::{
    authorize_target, department_in_scope, Action, Change, FieldChange, Receipt, TrustedEditContext,
};
use crate::storage::{composer_draft_repository, now_ms};
use crate::workflow::{
    AgentConfig, Definition, Department, ManagementRole, Node, NodeConfig, WorkflowPermissionMode,
};
use crate::workflow_management::Instance;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(super) struct Editor<'a> {
    pub c: &'a Connection,
    pub instance: &'a Instance,
    pub actor: &'a Node,
    pub context: &'a TrustedEditContext,
    pub available_models: &'a std::collections::HashSet<String>,
    pub original: &'a Definition,
    pub definition: &'a mut Definition,
    pub receipt: &'a mut Receipt,
}
fn mode_name(mode: &WorkflowPermissionMode) -> &'static str {
    match mode {
        WorkflowPermissionMode::Default => "default",
        WorkflowPermissionMode::Custom => "custom",
        WorkflowPermissionMode::Full => "full",
    }
}
fn parse_mode(mode: &str) -> Result<WorkflowPermissionMode, String> {
    match mode {
        "default" => Ok(WorkflowPermissionMode::Default),
        "custom" => Ok(WorkflowPermissionMode::Custom),
        "full" => Ok(WorkflowPermissionMode::Full),
        _ => Err("Unknown conversation permission mode".into()),
    }
}
fn member_value(node: Option<&Node>) -> Value {
    let Some(node) = node else {
        return Value::Null;
    };
    let NodeConfig::Agent(config) = &node.config;
    json!({"name":node.name,"task":config.task,"receives":config.receives,"delivers":config.delivers,"rank":node.rank,"managementRole":node.management_role,"departmentId":node.department_id,"modelConfigId":config.model_config_id,"permissionMode":config.permission_mode})
}
fn department_value(department: Option<&Department>) -> Value {
    department
        .map(|d| json!({"name":d.name,"parentId":d.parent_id}))
        .unwrap_or(Value::Null)
}
impl Editor<'_> {
    fn model(&self, id: &str) -> Result<(), String> {
        if !self.available_models.contains(id) {
            return Err("Organization model is unavailable; choose an executable model from the current model directory".into());
        }
        Ok(())
    }
    fn permission(&self, mode: &WorkflowPermissionMode) -> Result<(), String> {
        if !self.context.allowed_permission_modes.contains(mode) {
            return Err("organization_permission_ceiling: Choose an explicit permissionMode within the caller's current execution permissions".into());
        }
        Ok(())
    }
    fn member_authority(&self, before: Option<&Node>, after: &Node) -> Result<(), String> {
        if let Some(before) = before {
            // Authority must exist in both the original transaction snapshot and the staged tree;
            // an earlier reparent in the batch cannot launder an out-of-scope target into scope.
            let original = self
                .original
                .nodes
                .iter()
                .find(|n| n.id == before.id)
                .unwrap_or(before);
            authorize_target(
                self.actor,
                Some(original),
                original.rank,
                original.department_id.as_deref(),
                &self.original.departments,
            )?;
        }
        authorize_target(
            self.actor,
            before,
            after.rank,
            after.department_id.as_deref(),
            &self.definition.departments,
        )?;
        if after.management_role == ManagementRole::DepartmentAdmin && after.department_id.is_none()
        {
            return Err("Department administrators must belong to a department".into());
        }
        if self.actor.management_role == ManagementRole::DepartmentAdmin
            && after.management_role == ManagementRole::OrganizationAdmin
        {
            return Err(
                "A department administrator cannot delegate organization administrator authority"
                    .into(),
            );
        }
        Ok(())
    }
    fn department_authority(
        &self,
        id: Option<&str>,
        departments: &[Department],
    ) -> Result<(), String> {
        if id.is_some_and(|id| !departments.iter().any(|d| d.id == id)) {
            return Err("Organization department is unavailable".into());
        }
        if self.actor.management_role == ManagementRole::DepartmentAdmin
            && !department_in_scope(
                self.actor
                    .department_id
                    .as_deref()
                    .ok_or("organization_management_denied")?,
                id,
                departments,
            )
        {
            return Err("organization_management_denied".into());
        }
        Ok(())
    }
    fn chat(&self, node_id: &str) -> Result<Option<String>, String> {
        self.c.query_row("SELECT conversation_id FROM workflow_instance_bindings WHERE instance_id=?1 AND node_id=?2",params![self.instance.id,node_id],|row|row.get(0)).optional().map_err(error)
    }
    fn label(&self, field: &str, value: &Value, before: bool) -> Result<Option<String>, String> {
        let Some(id) = value.as_str() else {
            return Ok(None);
        };
        if field == "modelConfigId" {
            return self
                .c
                .query_row("SELECT display_name FROM models WHERE id=?1", [id], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(error);
        }
        if field == "departmentId" || field == "parentId" {
            let departments = if before {
                &self.original.departments
            } else {
                &self.definition.departments
            };
            let mut names = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut current = Some(id);
            while let Some(id) = current {
                if !seen.insert(id) {
                    return Ok(None);
                }
                let Some(department) = departments.iter().find(|department| department.id == id)
                else {
                    return Ok(None);
                };
                names.push(department.name.trim());
                current = department.parent_id.as_deref();
            }
            names.reverse();
            return Ok(Some(names.join("/")));
        }
        Ok(None)
    }
    fn record(
        &mut self,
        action: &Action,
        entity: (&str, &str, &str),
        chat: Option<String>,
        before: Value,
        after: Value,
    ) -> Result<(), String> {
        let (entity_type, id, name) = entity;
        let keys: &[&str] = if entity_type == "member" {
            &[
                "name",
                "task",
                "receives",
                "delivers",
                "rank",
                "managementRole",
                "departmentId",
                "modelConfigId",
                "permissionMode",
            ]
        } else {
            &["name", "parentId"]
        };
        let mut fields = Vec::new();
        for field in keys {
            let old = before.get(*field).cloned().unwrap_or(Value::Null);
            let new = after.get(*field).cloned().unwrap_or(Value::Null);
            // Include all fields for creation/removal, including an explicit root department.
            if old != new || before.is_null() || after.is_null() {
                fields.push(FieldChange {
                    field: (*field).into(),
                    before_label: self.label(field, &old, true)?,
                    after_label: self.label(field, &new, false)?,
                    before: old,
                    after: new,
                });
            }
        }
        if let Some(chat) = &chat {
            self.receipt.affected_conversation_ids.push(chat.clone());
        }
        let entity_name = if entity_type == "department" {
            let id = Value::String(id.into());
            self.label("departmentId", &id, false)?
                .or(self.label("departmentId", &id, true)?)
                .unwrap_or_else(|| name.into())
        } else {
            name.into()
        };
        self.receipt.changes.push(Change {
            action: action.name().into(),
            entity_type: entity_type.into(),
            entity_id: id.into(),
            entity_name,
            conversation_id: chat,
            fields,
        });
        Ok(())
    }
    fn configuration_event(&self, node_id: &str, kind: &str) -> Result<(), String> {
        self.c.execute("INSERT INTO workflow_mail_events(instance_id,source_node_id,target_node_id,kind,created_at) VALUES(?1,?2,?3,?4,?5)",params![self.instance.id,self.actor.id,node_id,kind,now_ms()]).map_err(error)?;
        Ok(())
    }
    pub fn apply(&mut self, action: &Action) -> Result<bool, String> {
        match action {
            Action::AddMember {
                name,
                task,
                receives,
                delivers,
                rank,
                management_role,
                department_id,
                model_config_id,
                permission_mode,
            } => {
                if self.definition.nodes.len() >= 128 {
                    return Err("Organization has reached its 128 member limit".into());
                }
                let department_id = department_id.resolve(self.actor.department_id.as_deref());
                let model = model_config_id
                    .clone()
                    .unwrap_or_else(|| self.context.current_model_id.clone());
                self.model(&model)?;
                let permission=permission_mode.clone().or_else(||self.context.default_permission_mode.clone()).ok_or("organization_permission_not_representable: Specify a permissionMode that fits the caller's execution permissions")?;
                self.permission(&permission)?;
                let node = Node {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.clone(),
                    x: 0.,
                    y: 0.,
                    rank: *rank,
                    management_role: management_role.clone(),
                    department_id,
                    config: NodeConfig::Agent(AgentConfig {
                        permission_mode: permission.clone(),
                        model_config_id: Some(model.clone()),
                        receives: receives.clone(),
                        task: task.clone(),
                        delivers: delivers.clone(),
                    }),
                };
                self.member_authority(None, &node)?;
                let chat = uuid::Uuid::new_v4().to_string();
                let now = now_ms();
                self.c.execute("INSERT INTO conversations(id,project_id,model_id,title,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",params![chat,self.instance.project_id,model,name,now]).map_err(error)?;
                composer_draft_repository::set_composer_configuration(
                    self.c,
                    &chat,
                    Some(&model),
                    mode_name(&permission),
                    now,
                )
                .map_err(error)?;
                self.c.execute("INSERT INTO workflow_instance_bindings(instance_id,node_id,conversation_id,membership_id) VALUES(?1,?2,?3,?4)",params![self.instance.id,node.id,chat,uuid::Uuid::new_v4().to_string()]).map_err(error)?;
                self.definition.nodes.push(node.clone());
                self.record(
                    action,
                    ("member", &node.id, &node.name),
                    Some(chat),
                    Value::Null,
                    member_value(Some(&node)),
                )?;
                Ok(true)
            }
            Action::UpdateMember {
                member_id,
                name,
                task,
                receives,
                delivers,
                rank,
                management_role,
                department_id,
                model_config_id,
                permission_mode,
            } => {
                let before = self
                    .definition
                    .nodes
                    .iter()
                    .find(|n| &n.id == member_id)
                    .cloned()
                    .ok_or("Organization member is unavailable")?;
                let mut after = before.clone();
                if let Some(name) = name {
                    after.name = name.clone();
                }
                if let Some(rank) = rank {
                    after.rank = *rank;
                }
                if let Some(role) = management_role {
                    after.management_role = role.clone();
                }
                after.department_id = department_id.resolve(before.department_id.as_deref());
                let NodeConfig::Agent(config) = &mut after.config;
                if let Some(task) = task {
                    config.task = task.clone();
                }
                if let Some(receives) = receives {
                    config.receives = receives.clone();
                }
                if let Some(delivers) = delivers {
                    config.delivers = delivers.clone();
                }
                if let Some(model) = model_config_id {
                    config.model_config_id = Some(model.clone());
                }
                if let Some(mode) = permission_mode {
                    config.permission_mode = mode.clone();
                }
                self.model(
                    config
                        .model_config_id
                        .as_deref()
                        .ok_or("Organization member has no configured model")?,
                )?;
                self.permission(&config.permission_mode)?;
                self.member_authority(Some(&before), &after)?;
                let chat = self.chat(member_id)?;
                let mut before_value = member_value(Some(&before));
                let after_value = member_value(Some(&after));
                let NodeConfig::Agent(before_config) = &before.config;
                let (current_model, current_mode): (Option<String>, Option<String>) = if let Some(
                    chat_id,
                ) =
                    chat.as_deref()
                {
                    self.c.query_row("SELECT COALESCE(d.model_id,c.model_id),d.permission_mode FROM conversations c LEFT JOIN composer_drafts d ON d.scope_id=c.id WHERE c.id=?1",[chat_id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(error)?
                } else {
                    (
                        before_config.model_config_id.clone(),
                        Some(mode_name(&before_config.permission_mode).into()),
                    )
                };
                let desired_mode = permission_mode.clone().map(Ok).unwrap_or_else(|| {
                    current_mode
                        .as_deref()
                        .map(parse_mode)
                        .unwrap_or_else(|| Ok(before_config.permission_mode.clone()))
                })?;
                // The next-run composer is authoritative even when this edit changes only prose.
                // Reject an excessive retained preset instead of silently downgrading it.
                self.permission(&desired_mode)?;
                if model_config_id.is_some() || permission_mode.is_some() {
                    let chat_id = chat
                        .as_deref()
                        .ok_or("Organization member has no bound conversation")?;
                    let desired_model = model_config_id
                        .clone()
                        .or_else(|| current_model.clone())
                        .or_else(|| before_config.model_config_id.clone());
                    let model_changed = model_config_id.as_ref().is_some_and(|id| {
                        current_model.as_ref() != Some(id)
                            || before_config.model_config_id.as_ref() != Some(id)
                    });
                    let permission_changed = permission_mode.as_ref().is_some_and(|mode| {
                        current_mode.as_deref() != Some(mode_name(mode))
                            || before_config.permission_mode != *mode
                    });
                    composer_draft_repository::set_composer_configuration(
                        self.c,
                        chat_id,
                        desired_model.as_deref(),
                        mode_name(&desired_mode),
                        now_ms(),
                    )
                    .map_err(error)?;
                    if model_changed {
                        if current_model.as_ref() != model_config_id.as_ref() {
                            before_value["modelConfigId"] = json!(current_model);
                        }
                        self.configuration_event(member_id, "member_model_changed")?;
                    }
                    if permission_changed {
                        if current_mode.as_deref() != permission_mode.as_ref().map(mode_name) {
                            before_value["permissionMode"] = json!(current_mode);
                        }
                        self.configuration_event(member_id, "member_permissions_changed")?;
                    }
                }
                let geometry = before.department_id != after.department_id;
                *self
                    .definition
                    .nodes
                    .iter_mut()
                    .find(|n| &n.id == member_id)
                    .unwrap() = after.clone();
                self.record(
                    action,
                    ("member", member_id, &after.name),
                    chat,
                    before_value,
                    after_value,
                )?;
                Ok(geometry)
            }
            Action::RemoveMember { member_id } => {
                let node = self
                    .definition
                    .nodes
                    .iter()
                    .find(|n| &n.id == member_id)
                    .cloned()
                    .ok_or("Organization member is unavailable")?;
                let original = self
                    .original
                    .nodes
                    .iter()
                    .find(|n| n.id == node.id)
                    .unwrap_or(&node);
                authorize_target(
                    self.actor,
                    Some(original),
                    original.rank,
                    original.department_id.as_deref(),
                    &self.original.departments,
                )?;
                authorize_target(
                    self.actor,
                    Some(&node),
                    node.rank,
                    node.department_id.as_deref(),
                    &self.definition.departments,
                )?;
                let chat = self.chat(member_id)?;
                self.c.execute("DELETE FROM workflow_instance_bindings WHERE instance_id=?1 AND node_id=?2",params![self.instance.id,member_id]).map_err(error)?;
                self.definition.nodes.retain(|n| &n.id != member_id);
                self.record(
                    action,
                    ("member", member_id, &node.name),
                    chat,
                    member_value(Some(&node)),
                    Value::Null,
                )?;
                Ok(true)
            }
            Action::AddDepartment { name, parent_id } => {
                if self.definition.departments.len() >= 64 {
                    return Err("Organization has reached its 64 department limit".into());
                }
                let fallback = if self.actor.management_role == ManagementRole::DepartmentAdmin {
                    self.actor.department_id.as_deref()
                } else {
                    None
                };
                let parent_id = parent_id.resolve(fallback);
                self.department_authority(parent_id.as_deref(), &self.definition.departments)?;
                let department = Department {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.clone(),
                    parent_id,
                    x: 0.,
                    y: 0.,
                    width: 260.,
                    height: 80.,
                };
                self.definition.departments.push(department.clone());
                self.record(
                    action,
                    ("department", &department.id, &department.name),
                    None,
                    Value::Null,
                    department_value(Some(&department)),
                )?;
                Ok(true)
            }
            Action::UpdateDepartment {
                department_id,
                name,
                parent_id,
            } => {
                let before = self
                    .definition
                    .departments
                    .iter()
                    .find(|d| &d.id == department_id)
                    .cloned()
                    .ok_or("Organization department is unavailable")?;
                self.department_authority(Some(department_id), &self.original.departments)?;
                self.department_authority(Some(department_id), &self.definition.departments)?;
                let mut after = before.clone();
                if let Some(name) = name {
                    after.name = name.clone();
                }
                after.parent_id = parent_id.resolve(before.parent_id.as_deref());
                let moved = after.parent_id != before.parent_id;
                if moved {
                    self.department_authority(
                        after.parent_id.as_deref(),
                        &self.definition.departments,
                    )?;
                    if department_in_scope(
                        department_id,
                        after.parent_id.as_deref(),
                        &self.definition.departments,
                    ) {
                        return Err("Department hierarchy cannot contain a cycle".into());
                    }
                    let affected: Vec<Node> = self
                        .definition
                        .nodes
                        .iter()
                        .filter(|node| {
                            department_in_scope(
                                department_id,
                                node.department_id.as_deref(),
                                &self.definition.departments,
                            )
                        })
                        .cloned()
                        .collect();
                    for node in &affected {
                        let original = self
                            .original
                            .nodes
                            .iter()
                            .find(|n| n.id == node.id)
                            .unwrap_or(node);
                        authorize_target(
                            self.actor,
                            Some(original),
                            original.rank,
                            original.department_id.as_deref(),
                            &self.original.departments,
                        )?;
                        authorize_target(
                            self.actor,
                            Some(node),
                            node.rank,
                            node.department_id.as_deref(),
                            &self.definition.departments,
                        )?;
                    }
                    self.definition
                        .departments
                        .iter_mut()
                        .find(|d| &d.id == department_id)
                        .unwrap()
                        .parent_id = after.parent_id.clone();
                    for node in &affected {
                        authorize_target(
                            self.actor,
                            Some(node),
                            node.rank,
                            node.department_id.as_deref(),
                            &self.definition.departments,
                        )?;
                    }
                }
                *self
                    .definition
                    .departments
                    .iter_mut()
                    .find(|d| &d.id == department_id)
                    .unwrap() = after.clone();
                self.record(
                    action,
                    ("department", department_id, &after.name),
                    None,
                    department_value(Some(&before)),
                    department_value(Some(&after)),
                )?;
                Ok(moved)
            }
            Action::RemoveDepartment { department_id } => {
                let department = self
                    .definition
                    .departments
                    .iter()
                    .find(|d| &d.id == department_id)
                    .cloned()
                    .ok_or("Organization department is unavailable")?;
                self.department_authority(Some(department_id), &self.original.departments)?;
                self.department_authority(Some(department_id), &self.definition.departments)?;
                if self
                    .definition
                    .nodes
                    .iter()
                    .any(|n| n.department_id.as_deref() == Some(department_id))
                    || self
                        .definition
                        .departments
                        .iter()
                        .any(|d| d.parent_id.as_deref() == Some(department_id))
                {
                    return Err("Only an empty department without members or child departments can be removed".into());
                }
                self.definition
                    .departments
                    .retain(|d| &d.id != department_id);
                self.record(
                    action,
                    ("department", department_id, &department.name),
                    None,
                    department_value(Some(&department)),
                    Value::Null,
                )?;
                Ok(true)
            }
        }
    }
}
