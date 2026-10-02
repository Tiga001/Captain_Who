//! Organization authoring contract for a freely communicating team of independent conversations.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_NODES: usize = 128;
const MAX_DEPARTMENTS: usize = 64;
const MAX_NAME_BYTES: usize = 512;
const MAX_TEXT_BYTES: usize = 128_000;
const MAX_POSITION: f64 = 100_000.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowPermissionMode {
    Default,
    Custom,
    Full,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagementRole {
    #[default]
    Member,
    OrganizationAdmin,
    DepartmentAdmin,
}
fn default_rank() -> u8 {
    1
}
fn is_default_rank(rank: &u8) -> bool {
    *rank == default_rank()
}
fn is_member(role: &ManagementRole) -> bool {
    *role == ManagementRole::Member
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConfig {
    pub permission_mode: WorkflowPermissionMode,
    pub model_config_id: Option<String>,
    pub receives: String,
    pub task: String,
    pub delivers: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NodeConfig {
    Agent(AgentConfig),
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_rank", skip_serializing_if = "is_default_rank")]
    pub rank: u8,
    #[serde(default, skip_serializing_if = "is_member")]
    pub management_role: ManagementRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub department_id: Option<String>,
    #[serde(flatten)]
    pub config: NodeConfig,
}
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let obj = value
            .as_object_mut()
            .ok_or_else(|| serde::de::Error::custom("Invalid node"))?;
        let mut take = |key| {
            obj.remove(key)
                .ok_or_else(|| serde::de::Error::custom("Missing node field"))
        };
        let id = serde_json::from_value(take("id")?).map_err(serde::de::Error::custom)?;
        let name = serde_json::from_value(take("name")?).map_err(serde::de::Error::custom)?;
        let x = serde_json::from_value(take("x")?).map_err(serde::de::Error::custom)?;
        let y = serde_json::from_value(take("y")?).map_err(serde::de::Error::custom)?;
        let rank = obj
            .remove("rank")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_else(default_rank);
        let management_role = obj
            .remove("managementRole")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        let department_id = obj
            .remove("departmentId")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .flatten();
        if obj.get("kind").and_then(|v| v.as_str()) == Some("agent")
            && (!obj.contains_key("permissionMode") || !obj.contains_key("modelConfigId"))
        {
            return Err(serde::de::Error::custom("Missing agent configuration"));
        }
        let config = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            id,
            name,
            x,
            y,
            rank,
            management_role,
            department_id,
            config,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Department {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Node {
    #[cfg(test)]
    pub fn agent_mut(&mut self) -> &mut AgentConfig {
        match &mut self.config {
            NodeConfig::Agent(agent) => agent,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub background: String,
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub departments: Vec<Department>,
    pub viewport: Viewport,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub code: String,
    pub subject: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub definition: Definition,
    #[serde(default)]
    pub enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
    pub issues: Vec<Issue>,
}
#[derive(Debug)]
pub enum Request {
    List,
    Manage(crate::workflow_management::Request),
    SaveWithDraft {
        definition: Definition,
        expected_revision: u64,
        expected_draft_revision: Option<u64>,
    },
    Validate {
        definition: Definition,
    },
    Save {
        definition: Definition,
        expected_revision: u64,
    },
    Delete {
        id: String,
        expected_revision: u64,
    },
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if matches!(
            value.get("operation").and_then(|op| op.as_str()),
            Some(
                "listInstances"
                    | "saveInstance"
                    | "deleteInstance"
                    | "setInstanceEnabled"
                    | "saveDraft"
                    | "deleteDraft"
                    | "duplicate"
            )
        ) {
            return serde_json::from_value(value)
                .map(Self::Manage)
                .map_err(serde::de::Error::custom);
        }
        #[derive(Deserialize)]
        #[serde(tag = "operation", rename_all = "camelCase", deny_unknown_fields)]
        enum WireRequest {
            List {},
            Validate {
                definition: Definition,
            },
            Save {
                definition: Definition,
                #[serde(default, rename = "expectedDraftRevision")]
                expected_draft_revision: Option<u64>,
                #[serde(rename = "expectedRevision")]
                expected_revision: u64,
            },
            Delete {
                id: String,
                #[serde(rename = "expectedRevision")]
                expected_revision: u64,
            },
        }
        Ok(
            match serde_json::from_value::<WireRequest>(value).map_err(serde::de::Error::custom)? {
                WireRequest::List {} => Self::List,
                WireRequest::Validate { definition } => Self::Validate { definition },
                WireRequest::Save {
                    definition,
                    expected_revision,
                    expected_draft_revision,
                } => {
                    if expected_draft_revision.is_some() {
                        Self::SaveWithDraft {
                            definition,
                            expected_revision,
                            expected_draft_revision,
                        }
                    } else {
                        Self::Save {
                            definition,
                            expected_revision,
                        }
                    }
                }
                WireRequest::Delete {
                    id,
                    expected_revision,
                } => Self::Delete {
                    id,
                    expected_revision,
                },
            },
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvalidRecordReason {
    IncompatibleDefinition,
    InvalidDefinition,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidRecord {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub updated_at: i64,
    pub reason: InvalidRecordReason,
}
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub records: Vec<Record>,
    pub issues: Vec<Issue>,
    pub instances: Vec<crate::workflow_management::Instance>,
    pub drafts: Vec<crate::workflow_management::EditingDraft>,
    pub affected_conversation_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_records: Vec<InvalidRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalid_drafts: Vec<crate::workflow_management::InvalidEditingDraft>,
}
fn issue(out: &mut Vec<Issue>, code: &str, subject: &str) {
    out.push(Issue {
        code: code.into(),
        subject: subject.into(),
    });
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.trim() == id && id.len() <= 256 && !id.chars().any(char::is_control)
}
/// Comparison only: preserve the member's display name and meaningful internal spaces.
pub fn member_name_key(name: &str) -> String {
    name.trim().chars().flat_map(char::to_lowercase).collect()
}
impl Definition {
    /// Required for writes. Old duplicate names stay readable so the user can rename them.
    pub fn validate_unique_member_names(&self) -> Result<(), String> {
        let mut names = HashSet::new();
        for node in &self.nodes {
            let key = member_name_key(&node.name);
            if !key.is_empty() && !names.insert(key) {
                return Err("organization_duplicate_member_name: Every member needs a distinct name; leading/trailing whitespace and letter case do not distinguish members".into());
            }
        }
        Ok(())
    }

    /// Names form readable department paths. Siblings must remain unambiguous on writes.
    pub fn validate_department_names(&self) -> Result<(), String> {
        let mut names = HashSet::new();
        for department in &self.departments {
            if department.name.contains('/') {
                return Err("organization_department_name_separator: Department names cannot contain '/'; use nested departments instead".into());
            }
            let key = member_name_key(&department.name);
            if !key.is_empty() && !names.insert((department.parent_id.as_deref(), key)) {
                return Err("organization_duplicate_department_name: Departments with the same parent must have distinct names; leading/trailing whitespace and letter case do not distinguish departments".into());
            }
        }
        Ok(())
    }

    /// Reject malformed contracts while retaining incomplete authoring choices as issues.
    pub fn validate(&self, available_models: &HashSet<String>) -> Result<Vec<Issue>, String> {
        if self.schema_version != 1
            || !valid_id(&self.id)
            || self.nodes.len() > MAX_NODES
            || self.departments.len() > MAX_DEPARTMENTS
        {
            return Err("Unsupported organization version, identifier or team size".into());
        }
        if self.name.len() > MAX_NAME_BYTES
            || self.description.len() > 8_000
            || self.background.len() > MAX_TEXT_BYTES
        {
            return Err("Organization text exceeds its size limit".into());
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 2_000_000 {
            return Err("Organization exceeds 2 MB".into());
        }
        if !self.viewport.x.is_finite()
            || !self.viewport.y.is_finite()
            || self.viewport.x.abs() > MAX_POSITION
            || self.viewport.y.abs() > MAX_POSITION
            || !(0.25..=2.0).contains(&self.viewport.zoom)
        {
            return Err("Invalid viewport".into());
        }
        let ids: HashSet<_> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        if ids.len() != self.nodes.len() || ids.iter().any(|id| !valid_id(id)) {
            return Err("Duplicate or invalid node identifiers".into());
        }
        let mut issues = Vec::new();
        let mut names = HashMap::<String, usize>::new();
        for node in &self.nodes {
            let key = member_name_key(&node.name);
            if !key.is_empty() {
                *names.entry(key).or_default() += 1;
            }
        }
        for node in &self.nodes {
            if names
                .get(&member_name_key(&node.name))
                .is_some_and(|count| *count > 1)
            {
                issue(&mut issues, "node_name_duplicate", &node.id);
            }
        }
        let departments: HashMap<_, _> = self
            .departments
            .iter()
            .map(|department| (department.id.as_str(), department))
            .collect();
        if departments.len() != self.departments.len()
            || departments
                .keys()
                .any(|id| !valid_id(id) || ids.contains(id))
        {
            return Err("Duplicate or invalid department identifiers".into());
        }
        let mut department_names = HashMap::<(Option<&str>, String), usize>::new();
        for department in &self.departments {
            let key = member_name_key(&department.name);
            if !key.is_empty() {
                *department_names
                    .entry((department.parent_id.as_deref(), key))
                    .or_default() += 1;
            }
        }
        for department in &self.departments {
            if department.name.contains('/') {
                issue(&mut issues, "department_name_separator", &department.id);
            }
            if department_names
                .get(&(
                    department.parent_id.as_deref(),
                    member_name_key(&department.name),
                ))
                .is_some_and(|count| *count > 1)
            {
                issue(&mut issues, "department_name_duplicate", &department.id);
            }
        }
        for department in &self.departments {
            if department.name.len() > MAX_NAME_BYTES
                || !department.x.is_finite()
                || !department.y.is_finite()
                || department.x.abs() > MAX_POSITION
                || department.y.abs() > MAX_POSITION
                || !department.width.is_finite()
                || !department.height.is_finite()
                || department.width < 80.0
                || department.height < 64.0
                || department.width > MAX_POSITION
                || department.height > MAX_POSITION
            {
                return Err("Invalid department name or bounds".into());
            }
            let mut visited = HashSet::from([department.id.as_str()]);
            let mut parent_id = department.parent_id.as_deref();
            while let Some(id) = parent_id {
                let Some(parent) = departments.get(id) else {
                    return Err("Unknown parent department".into());
                };
                if !visited.insert(id) {
                    return Err("Department hierarchy contains a cycle".into());
                }
                parent_id = parent.parent_id.as_deref();
            }
            if department.name.trim().is_empty() {
                issue(&mut issues, "department_name", &department.id);
            }
        }
        if self.name.trim().is_empty() {
            issue(&mut issues, "name", &self.id);
        }
        if self.nodes.is_empty() {
            issue(&mut issues, "empty", &self.id);
        }
        for node in &self.nodes {
            if !(1..=99).contains(&node.rank) {
                return Err("Invalid member rank".into());
            }
            if node
                .department_id
                .as_deref()
                .is_some_and(|id| !departments.contains_key(id))
            {
                return Err("Unknown member department".into());
            }
            if node.management_role == ManagementRole::DepartmentAdmin
                && node.department_id.is_none()
            {
                issue(&mut issues, "department_admin_scope", &node.id);
            }
            if !node.x.is_finite()
                || !node.y.is_finite()
                || node.x.abs() > MAX_POSITION
                || node.y.abs() > MAX_POSITION
            {
                return Err("Invalid node position".into());
            }
            if node.name.len() > MAX_NAME_BYTES {
                return Err("Node name exceeds its size limit".into());
            }
            match &node.config {
                NodeConfig::Agent(agent) => {
                    if agent.model_config_id.as_ref().is_some_and(|id| {
                        id.trim().is_empty()
                            || id.trim() != id
                            || id.len() > 512
                            || id.chars().any(char::is_control)
                    }) {
                        return Err("Invalid model configuration identifier".into());
                    }
                    if [&agent.receives, &agent.task, &agent.delivers]
                        .iter()
                        .any(|v| v.len() > MAX_TEXT_BYTES)
                    {
                        return Err("Node text exceeds its size limit".into());
                    }
                    if node.name.trim().is_empty() || agent.task.trim().is_empty() {
                        issue(&mut issues, "task", &node.id);
                    }
                    match &agent.model_config_id {
                        None => issue(&mut issues, "node_model", &node.id),
                        Some(id) if !available_models.contains(id) => {
                            issue(&mut issues, "node_model_unavailable", &node.id)
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        Ok(issues)
    }
}
