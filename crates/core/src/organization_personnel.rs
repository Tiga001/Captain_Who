//! Model-facing organization edits and shared live authority checks.
use crate::workflow::{Department, ManagementRole, Node, WorkflowPermissionMode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::HashSet;

pub const MANAGEMENT_CAPABILITY: &str = "organization.management";

/// Omission preserves/inherits the placement; JSON null explicitly chooses organization root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OptionalId {
    #[default]
    Missing,
    Value(Option<String>),
}
impl OptionalId {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
    pub fn resolve(&self, fallback: Option<&str>) -> Option<String> {
        match self {
            Self::Missing => fallback.map(str::to_owned),
            Self::Value(value) => value.clone(),
        }
    }
    fn validate(&self) -> bool {
        match self {
            Self::Value(Some(id)) => valid_id(id),
            _ => true,
        }
    }
}
impl Serialize for OptionalId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Missing => serializer.serialize_none(),
            Self::Value(value) => value.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for OptionalId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<String>::deserialize(deserializer).map(Self::Value)
    }
}
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}
fn default_rank() -> u8 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Action {
    AddMember {
        name: String,
        task: String,
        #[serde(default)]
        receives: String,
        #[serde(default)]
        delivers: String,
        #[serde(default = "default_rank")]
        rank: u8,
        #[serde(default)]
        management_role: ManagementRole,
        #[serde(default, skip_serializing_if = "OptionalId::is_missing")]
        department_id: OptionalId,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        model_config_id: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        permission_mode: Option<WorkflowPermissionMode>,
    },
    UpdateMember {
        member_id: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        name: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        task: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        receives: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        delivers: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        rank: Option<u8>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        management_role: Option<ManagementRole>,
        #[serde(default, skip_serializing_if = "OptionalId::is_missing")]
        department_id: OptionalId,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        model_config_id: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        permission_mode: Option<WorkflowPermissionMode>,
    },
    RemoveMember {
        member_id: String,
    },
    AddDepartment {
        name: String,
        #[serde(default, skip_serializing_if = "OptionalId::is_missing")]
        parent_id: OptionalId,
    },
    UpdateDepartment {
        department_id: String,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "OptionalId::is_missing")]
        parent_id: OptionalId,
    },
    RemoveDepartment {
        department_id: String,
    },
}
impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::AddMember { .. } => "add_member",
            Self::UpdateMember { .. } => "update_member",
            Self::RemoveMember { .. } => "remove_member",
            Self::AddDepartment { .. } => "add_department",
            Self::UpdateDepartment { .. } => "update_department",
            Self::RemoveDepartment { .. } => "remove_department",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub reason: String,
    pub changes: Vec<Action>,
}
impl Input {
    pub fn validate(&self) -> Result<(), String> {
        if self.reason.trim().is_empty()
            || self.reason.chars().count() > 240
            || self.reason.chars().any(char::is_control)
            || self.changes.is_empty()
            || self.changes.len() > 32
        {
            return Err("Provide a short reason and 1 to 32 organization changes".into());
        }
        let mut targets = HashSet::new();
        for action in &self.changes {
            let valid = match action {
                Action::AddMember {
                    name,
                    task,
                    receives,
                    delivers,
                    rank,
                    department_id,
                    model_config_id,
                    ..
                } => {
                    valid_name(name)
                        && valid_task(task)
                        && valid_text(receives)
                        && valid_text(delivers)
                        && (1..=99).contains(rank)
                        && department_id.validate()
                        && model_config_id.as_ref().is_none_or(|id| valid_id(id))
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
                    valid_id(member_id)
                        && targets.insert(("member", member_id))
                        && name.as_ref().is_none_or(|v| valid_name(v))
                        && task.as_ref().is_none_or(|v| valid_task(v))
                        && receives.as_ref().is_none_or(|v| valid_text(v))
                        && delivers.as_ref().is_none_or(|v| valid_text(v))
                        && rank.is_none_or(|v| (1..=99).contains(&v))
                        && department_id.validate()
                        && model_config_id.as_ref().is_none_or(|v| valid_id(v))
                        && (name.is_some()
                            || task.is_some()
                            || receives.is_some()
                            || delivers.is_some()
                            || rank.is_some()
                            || management_role.is_some()
                            || !department_id.is_missing()
                            || model_config_id.is_some()
                            || permission_mode.is_some())
                }
                Action::RemoveMember { member_id } => {
                    valid_id(member_id) && targets.insert(("member", member_id))
                }
                Action::AddDepartment { name, parent_id } => {
                    valid_name(name) && parent_id.validate()
                }
                Action::UpdateDepartment {
                    department_id,
                    name,
                    parent_id,
                } => {
                    valid_id(department_id)
                        && targets.insert(("department", department_id))
                        && name.as_ref().is_none_or(|v| valid_name(v))
                        && parent_id.validate()
                        && (name.is_some() || !parent_id.is_missing())
                }
                Action::RemoveDepartment { department_id } => {
                    valid_id(department_id) && targets.insert(("department", department_id))
                }
            };
            if !valid {
                return Err("Invalid organization change; use valid fields and edit each existing entity at most once".into());
            }
        }
        Ok(())
    }
}
fn valid_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
fn valid_task(value: &str) -> bool {
    !value.trim().is_empty() && valid_text(value)
}
fn valid_text(value: &str) -> bool {
    value.len() <= 128_000
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

/// Resolved by the Host from the active run, never model-authored arguments or composer guesses.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedEditContext {
    pub current_model_id: String,
    pub permission_settings_fingerprint: String,
    pub default_permission_mode: Option<WorkflowPermissionMode>,
    pub allowed_permission_modes: Vec<WorkflowPermissionMode>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub conversation_id: String,
    pub source_run_id: String,
    pub tool_call_id: String,
    pub execution_version: String,
    pub expected_revision: u64,
    pub input: Input,
    pub model_input: Option<Value>,
    pub context: TrustedEditContext,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_label: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub action: String,
    pub entity_type: String,
    pub entity_id: String,
    pub entity_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub fields: Vec<FieldChange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub instance_id: String,
    pub organization_name: String,
    pub organization_revision: u64,
    pub changes: Vec<Change>,
    pub affected_conversation_ids: Vec<String>,
}
pub fn can_manage(role: &ManagementRole, department_id: Option<&str>) -> bool {
    matches!(role, ManagementRole::OrganizationAdmin)
        || (matches!(role, ManagementRole::DepartmentAdmin) && department_id.is_some())
}
pub fn department_in_scope(
    actor_department: &str,
    target_department: Option<&str>,
    departments: &[Department],
) -> bool {
    let mut current = target_department;
    let mut visited = HashSet::new();
    while let Some(id) = current {
        if !visited.insert(id) {
            return false;
        }
        let Some(department) = departments.iter().find(|d| d.id == id) else {
            return false;
        };
        if id == actor_department {
            return true;
        }
        current = department.parent_id.as_deref();
    }
    false
}
pub fn authorize_target(
    actor: &Node,
    target: Option<&Node>,
    new_rank: u8,
    target_department: Option<&str>,
    departments: &[Department],
) -> Result<(), String> {
    if !can_manage(&actor.management_role, actor.department_id.as_deref())
        || !(1..=99).contains(&new_rank)
        || new_rank >= actor.rank
        || target.is_some_and(|target| target.id == actor.id || target.rank >= actor.rank)
        || target_department.is_some_and(|id| !departments.iter().any(|d| d.id == id))
    {
        return Err("organization_management_denied".into());
    }
    if actor.management_role == ManagementRole::DepartmentAdmin {
        let department = actor
            .department_id
            .as_deref()
            .ok_or("organization_management_denied")?;
        if !department_in_scope(department, target_department, departments)
            || target.is_some_and(|target| {
                !department_in_scope(department, target.department_id.as_deref(), departments)
            })
        {
            return Err("organization_management_denied".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unified_edit_distinguishes_omission_null_and_preserved_fields() {
        let omitted:Input=serde_json::from_value(json!({"reason":"Arrange team","changes":[{"action":"update_member","memberId":"a","receives":""}]})).unwrap();
        assert!(omitted.validate().is_ok());
        let Action::UpdateMember { department_id, .. } = &omitted.changes[0] else {
            panic!()
        };
        assert_eq!(department_id.resolve(Some("old")), Some("old".into()));
        let explicit:Input=serde_json::from_value(json!({"reason":"Arrange team","changes":[{"action":"update_member","memberId":"a","departmentId":null}]})).unwrap();
        let Action::UpdateMember { department_id, .. } = &explicit.changes[0] else {
            panic!()
        };
        assert_eq!(department_id.resolve(Some("old")), None);
        assert_ne!(
            serde_json::to_value(omitted).unwrap(),
            serde_json::to_value(explicit).unwrap()
        );
    }
    #[test]
    fn unified_edit_rejects_old_tools_raw_geometry_null_text_and_empty_changes() {
        for change in [
            json!({"action":"add","name":"A","task":"T","rank":1}),
            json!({"action":"add_member","name":"A","task":"T","x":4}),
            json!({"action":"update_member","memberId":"a","task":null}),
            json!({"action":"update_member","memberId":"a","modelConfigId":null}),
        ] {
            assert!(serde_json::from_value::<Action>(change).is_err());
        }
        for change in [
            json!({"action":"update_member","memberId":"a"}),
            json!({"action":"update_member","memberId":"a","task":""}),
            json!({"action":"add_member","name":"","task":"T"}),
            json!({"action":"update_department","departmentId":"d"}),
        ] {
            let input: Input =
                serde_json::from_value(json!({"reason":"Edit","changes":[change]})).unwrap();
            assert!(input.validate().is_err());
        }
        assert!(serde_json::from_value::<Input>(json!({"reason":"Edit","actions":[]})).is_err());
    }
}
