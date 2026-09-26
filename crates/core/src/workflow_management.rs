//! Project-independent workflow instances and editing drafts; no execution admission API.
use crate::workflow::Definition;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub node_id: String,
    pub conversation_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingInput {
    pub node_id: String,
    pub conversation_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instance {
    pub id: String,
    pub template_id: String,
    pub template_revision: u64,
    pub name: String,
    pub color: String,
    /// Destination only for automatically created conversations; bindings remain cross-project.
    #[serde(default)]
    pub project_id: Option<String>,
    pub bindings: Vec<Binding>,
    pub revision: u64,
    pub updated_at: i64,
    pub needs_review: bool,
    pub running: bool,
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageInstance {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub project_names: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    pub template_id: String,
    pub usage_revision: String,
    pub instances: Vec<UsageInstance>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditingDraft {
    pub definition: Definition,
    pub base_revision: u64,
    pub revision: u64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidEditingDraft {
    pub id: String,
    pub name: String,
    pub base_revision: u64,
    pub revision: u64,
    pub updated_at: i64,
    pub reason: crate::workflow::InvalidRecordReason,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Request {
    ListInstances {},
    SaveInstance {
        id: String,
        template_id: String,
        name: String,
        color: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        project_id: Option<String>,
        bindings: Vec<BindingInput>,
        expected_revision: u64,
        expected_template_revision: u64,
    },
    SetInstanceEnabled {
        id: String,
        enabled: bool,
        expected_revision: u64,
    },
    DeleteInstance {
        id: String,
        expected_revision: u64,
    },
    SaveDraft {
        definition: Definition,
        expected_revision: u64,
        expected_draft_revision: u64,
    },
    DeleteDraft {
        id: String,
        expected_draft_revision: u64,
    },
    Duplicate {
        id: String,
        expected_revision: u64,
        new_id: String,
        name: String,
    },
}
