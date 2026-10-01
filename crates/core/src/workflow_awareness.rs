//! Read-only model queries. Workflow and mailbox ownership are supplied by the Host.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StateView {
    Topology,
    Runtime,
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
        }
    }
}

fn validate_id(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|value| {
        value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control)
    }) {
        return Err("Invalid workflow query identifier".into());
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
            return Err("Invalid workflow mailbox page".into());
        }
        validate_id(self.message_id.as_deref())?;
        validate_id(self.input_id.as_deref())
    }
}
