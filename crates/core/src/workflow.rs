//! Workflow authoring contract for a freely communicating team of independent conversations.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_NODES: usize = 128;
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
    User {
        #[serde(default)]
        task: String,
    },
}
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
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
            config,
        })
    }
}
impl Node {
    pub fn is_agent(&self) -> bool {
        matches!(self.config, NodeConfig::Agent(_))
    }
    pub fn is_participant(&self) -> bool {
        true
    }
    #[cfg(test)]
    pub fn agent_mut(&mut self) -> &mut AgentConfig {
        match &mut self.config {
            NodeConfig::Agent(agent) => agent,
            _ => panic!("Expected agent"),
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
    SaveWithUsage {
        definition: Definition,
        expected_revision: u64,
        expected_usage_revision: Option<String>,
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
                #[serde(default, rename = "expectedUsageRevision")]
                expected_usage_revision: Option<String>,
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
                    expected_usage_revision,
                    expected_draft_revision,
                } => {
                    if expected_usage_revision.is_some() || expected_draft_revision.is_some() {
                        Self::SaveWithUsage {
                            definition,
                            expected_revision,
                            expected_usage_revision,
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
    pub usages: Vec<crate::workflow_management::Usage>,
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
impl Definition {
    /// Reject malformed contracts while retaining incomplete authoring choices as issues.
    pub fn validate(&self, available_models: &HashSet<String>) -> Result<Vec<Issue>, String> {
        if self.schema_version != 1 || !valid_id(&self.id) || self.nodes.len() > MAX_NODES {
            return Err("Unsupported workflow version, identifier or team size".into());
        }
        if self.name.len() > MAX_NAME_BYTES
            || self.description.len() > 8_000
            || self.background.len() > MAX_TEXT_BYTES
        {
            return Err("Workflow text exceeds its size limit".into());
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 2_000_000 {
            return Err("Workflow exceeds 2 MB".into());
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
        if self.name.trim().is_empty() {
            issue(&mut issues, "name", &self.id);
        }
        if self.nodes.is_empty() {
            issue(&mut issues, "empty", &self.id);
        }
        for node in &self.nodes {
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
                NodeConfig::User { task } => {
                    if task.len() > MAX_TEXT_BYTES {
                        return Err("User task exceeds its size limit".into());
                    }
                    if task.trim().is_empty() {
                        issue(&mut issues, "userTask", &node.id);
                    }
                }
            }
        }
        Ok(issues)
    }
}
