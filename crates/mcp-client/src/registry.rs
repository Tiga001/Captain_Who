use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{config_digest, McpConfigDigest, McpError, McpServerConfig, McpServerId};

const REGISTRY_CHANGE_CAPACITY: usize = 128;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryEntry {
    pub config: McpServerConfig,
    pub config_digest: McpConfigDigest,
    pub revision: u64,
}

impl fmt::Debug for McpRegistryEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpRegistryEntry")
            .field("server_id", &self.config.id)
            .field("enabled", &self.config.enabled)
            .field("scope", &self.config.scope)
            .field("trust", &self.config.trust)
            .field("config_digest", &self.config_digest)
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpRegistryChangeKind {
    Added,
    Updated,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryChange {
    pub revision: u64,
    pub kind: McpRegistryChangeKind,
    pub server_id: McpServerId,
    pub enabled: bool,
    pub config_digest: McpConfigDigest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpRegistryMutation {
    Added(McpRegistryEntry),
    Updated(McpRegistryEntry),
    Unchanged(McpRegistryEntry),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpRegistrySubscriptionError {
    Lagged { skipped: u64 },
    Closed,
}

pub struct McpRegistrySubscription {
    receiver: broadcast::Receiver<McpRegistryChange>,
}

impl fmt::Debug for McpRegistrySubscription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpRegistrySubscription")
            .finish_non_exhaustive()
    }
}

impl McpRegistrySubscription {
    pub async fn recv(&mut self) -> Result<McpRegistryChange, McpRegistrySubscriptionError> {
        match self.receiver.recv().await {
            Ok(change) => Ok(change),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                Err(McpRegistrySubscriptionError::Lagged { skipped })
            }
            Err(broadcast::error::RecvError::Closed) => Err(McpRegistrySubscriptionError::Closed),
        }
    }
}

pub trait McpRegistry: Send + Sync {
    fn add(&self, config: McpServerConfig) -> Result<McpRegistryEntry, McpError>;
    fn upsert(&self, config: McpServerConfig) -> Result<McpRegistryMutation, McpError>;
    /// Remove only the stored record.
    ///
    /// Hosts with a live `McpConnectionManager` must use
    /// `McpConnectionManager::remove_server` so the peer is stopped and reaped
    /// before this storage mutation becomes visible.
    fn remove(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError>;
    fn get(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError>;
    fn list(&self) -> Result<Vec<McpRegistryEntry>, McpError>;
    fn subscribe(&self) -> McpRegistrySubscription;
}

#[derive(Debug, Default)]
struct RegistryState {
    revision: u64,
    entries: BTreeMap<McpServerId, McpRegistryEntry>,
}

#[derive(Debug)]
pub struct InMemoryMcpRegistry {
    state: RwLock<RegistryState>,
    changes: broadcast::Sender<McpRegistryChange>,
}

impl Default for InMemoryMcpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryMcpRegistry {
    pub fn new() -> Self {
        let (changes, _) = broadcast::channel(REGISTRY_CHANGE_CAPACITY);
        Self {
            state: RwLock::new(RegistryState::default()),
            changes,
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    fn next_revision(state: &mut RegistryState) -> Result<u64, McpError> {
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or_else(|| McpError::protocol("MCP registry revision exhausted"))?;
        Ok(state.revision)
    }

    fn publish(&self, change: McpRegistryChange) {
        let _ = self.changes.send(change);
    }
}

impl McpRegistry for InMemoryMcpRegistry {
    fn add(&self, config: McpServerConfig) -> Result<McpRegistryEntry, McpError> {
        let digest = config_digest(&config)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| McpError::protocol("MCP registry write lock is unavailable"))?;
        if state.entries.contains_key(&config.id) {
            return Err(McpError::config(
                "MCP registry already contains this server ID",
            ));
        }
        let revision = Self::next_revision(&mut state)?;
        let entry = McpRegistryEntry {
            config,
            config_digest: digest,
            revision,
        };
        state.entries.insert(entry.config.id, entry.clone());
        let change = McpRegistryChange {
            revision,
            kind: McpRegistryChangeKind::Added,
            server_id: entry.config.id,
            enabled: entry.config.enabled,
            config_digest: entry.config_digest.clone(),
        };
        self.publish(change);
        Ok(entry)
    }

    fn upsert(&self, config: McpServerConfig) -> Result<McpRegistryMutation, McpError> {
        let digest = config_digest(&config)?;
        let mut state = self
            .state
            .write()
            .map_err(|_| McpError::protocol("MCP registry write lock is unavailable"))?;
        if let Some(existing) = state.entries.get(&config.id) {
            if existing.config_digest == digest {
                return Ok(McpRegistryMutation::Unchanged(existing.clone()));
            }
        }
        let existed = state.entries.contains_key(&config.id);
        let revision = Self::next_revision(&mut state)?;
        let entry = McpRegistryEntry {
            config,
            config_digest: digest,
            revision,
        };
        state.entries.insert(entry.config.id, entry.clone());
        let kind = if existed {
            McpRegistryChangeKind::Updated
        } else {
            McpRegistryChangeKind::Added
        };
        let change = McpRegistryChange {
            revision,
            kind,
            server_id: entry.config.id,
            enabled: entry.config.enabled,
            config_digest: entry.config_digest.clone(),
        };
        self.publish(change);
        Ok(if existed {
            McpRegistryMutation::Updated(entry)
        } else {
            McpRegistryMutation::Added(entry)
        })
    }

    fn remove(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError> {
        let mut state = self
            .state
            .write()
            .map_err(|_| McpError::protocol("MCP registry write lock is unavailable"))?;
        let Some(mut removed) = state.entries.remove(&server_id) else {
            return Ok(None);
        };
        let revision = Self::next_revision(&mut state)?;
        removed.revision = revision;
        let change = McpRegistryChange {
            revision,
            kind: McpRegistryChangeKind::Removed,
            server_id,
            enabled: removed.config.enabled,
            config_digest: removed.config_digest.clone(),
        };
        self.publish(change);
        Ok(Some(removed))
    }

    fn get(&self, server_id: McpServerId) -> Result<Option<McpRegistryEntry>, McpError> {
        let state = self
            .state
            .read()
            .map_err(|_| McpError::protocol("MCP registry read lock is unavailable"))?;
        Ok(state.entries.get(&server_id).cloned())
    }

    fn list(&self) -> Result<Vec<McpRegistryEntry>, McpError> {
        let state = self
            .state
            .read()
            .map_err(|_| McpError::protocol("MCP registry read lock is unavailable"))?;
        Ok(state.entries.values().cloned().collect())
    }

    fn subscribe(&self) -> McpRegistrySubscription {
        McpRegistrySubscription {
            receiver: self.changes.subscribe(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{McpEnvBinding, McpServerScope, McpStdioConfig, McpTransportConfig, McpTrustLevel};

    fn config(id: McpServerId, display_name: &str) -> McpServerConfig {
        McpServerConfig {
            id,
            display_name: display_name.to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::UserApproved,
            enabled: true,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: PathBuf::from("/owned/fixture"),
                arguments: Vec::new(),
                cwd: PathBuf::from("/owned"),
                environment: vec![McpEnvBinding::SecretRef {
                    name: "TOKEN".to_string(),
                    secret_id: "opaque-secret-reference".to_string(),
                }],
            }),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 1_000,
            shutdown_timeout_ms: 1_000,
        }
    }

    #[tokio::test]
    async fn registry_is_stable_ordered_and_notifies_without_secret_material() {
        let registry = InMemoryMcpRegistry::new();
        let mut changes = registry.subscribe();
        let high = McpServerId::from_uuid(
            uuid::Uuid::parse_str("ffffffff-ffff-4fff-8fff-ffffffffffff").unwrap(),
        );
        let low = McpServerId::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").unwrap(),
        );
        registry.add(config(high, "duplicate")).unwrap();
        registry.add(config(low, "duplicate")).unwrap();
        let listed = registry.list().unwrap();
        assert_eq!(listed[0].config.id, low);
        assert_eq!(listed[1].config.id, high);

        let change = changes.recv().await.unwrap();
        let encoded = serde_json::to_string(&change).unwrap();
        assert!(!encoded.contains("opaque-secret-reference"));
        assert!(!encoded.contains("TOKEN"));
    }

    #[test]
    fn add_rejects_duplicate_ids_and_upsert_detects_no_change() {
        let registry = InMemoryMcpRegistry::new();
        let id = McpServerId::new();
        let original = config(id, "same");
        registry.add(original.clone()).unwrap();
        assert!(registry.add(original.clone()).is_err());
        assert!(matches!(
            registry.upsert(original).unwrap(),
            McpRegistryMutation::Unchanged(_)
        ));
    }
}
