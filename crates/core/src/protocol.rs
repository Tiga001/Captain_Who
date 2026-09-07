use crate::agent_graph::AgentCollaborationIdentity;
use crate::context::ContextCompactionSummary;
use crate::conversation_trace::{
    ConversationContextImageRef, ConversationModelContextItem, ConversationTraceAttachment,
    ConversationTurnTrace, ConversationTurnTraceItem,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::world_state::{AnchoredWorldStateRecord, WorldStateSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt::{Display, Formatter};

fn default_true() -> bool {
    true
}

pub(crate) fn deserialize_required_nullable<'de, D, T>(
    deserializer: D,
) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub const AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION: u32 = 1;

fn deserialize_file_change_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported Agent FileChange schema version",
        ))
    }
}

mod base;
mod checkpoint;
mod command_runtime;
mod error;
mod events;
mod identity_mcp;
mod image_file_change;
mod office_command;
mod session;
mod skills;

pub use base::*;
pub use checkpoint::*;
pub use command_runtime::*;
pub use error::*;
pub use events::*;
pub use identity_mcp::*;
pub use image_file_change::*;
pub use office_command::*;
pub use session::*;
pub use skills::*;

#[cfg(test)]
mod tests;
