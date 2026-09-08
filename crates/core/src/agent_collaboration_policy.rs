//! Host-owned collaboration policy for one logical run. The Host binds this policy to the run
//! and its admitted descendants; changing global settings cannot rewrite an existing task tree.

use crate::AgentResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationSettings {
    pub enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
}

impl Default for AgentCollaborationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            revision: 1,
            updated_at: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationSettingsUpdate {
    pub enabled: bool,
    pub expected_revision: u64,
}

/// Trusted policy already bound to this logical run. The runtime reads it once per execution
/// segment and checks it against the frozen checkpoint when resuming an approval.
pub trait AgentCollaborationPolicySource: Send + Sync {
    fn snapshot(&self) -> AgentResult<AgentCollaborationSettings>;
}

#[derive(Debug, Clone)]
pub struct FrozenAgentCollaborationPolicySource {
    settings: AgentCollaborationSettings,
}

impl FrozenAgentCollaborationPolicySource {
    pub fn new(settings: AgentCollaborationSettings) -> Self {
        Self { settings }
    }
}

impl AgentCollaborationPolicySource for FrozenAgentCollaborationPolicySource {
    fn snapshot(&self) -> AgentResult<AgentCollaborationSettings> {
        Ok(self.settings.clone())
    }
}
