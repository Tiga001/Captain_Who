use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{
    config_digest, McpConfigDigest, McpConfigEpoch, McpError, McpModelNamespace, McpServerConfig,
    McpServerId, McpServerScope, MCP_MODEL_NAMESPACE_MAX_BYTES,
};

const REGISTRY_CHANGE_CAPACITY: usize = 128;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRegistryEntry {
    pub config: McpServerConfig,
    /// Stable Host-owned model projection namespace. This value is never
    /// accepted from a Renderer and is preserved across configuration edits.
    pub model_namespace: McpModelNamespace,
    pub config_digest: McpConfigDigest,
    /// Opaque, non-reusable identity for this exact configuration incarnation.
    pub config_epoch: McpConfigEpoch,
    /// Monotonic Registry-wide event ordering revision.
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
            .field("model_namespace", &self.model_namespace)
            .field("config_digest", &self.config_digest)
            .field("config_epoch", &self.config_epoch)
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
    pub scope: McpServerScope,
    pub enabled: bool,
    pub config_digest: McpConfigDigest,
    /// For removal this is the removed configuration's epoch. A later re-add
    /// always receives a different epoch even if its digest is identical.
    pub config_epoch: McpConfigEpoch,
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
    /// Builds a subscription from a Registry-owned change broadcaster.
    ///
    /// This keeps the receiver field private while allowing storage adapters in
    /// a host crate to implement [`McpRegistry`] without exposing mutation
    /// access through the subscription API.
    pub fn from_sender(sender: &broadcast::Sender<McpRegistryChange>) -> Self {
        Self {
            receiver: sender.subscribe(),
        }
    }

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

/// Allocates the stable namespace used in model-visible MCP function names.
///
/// Callers must hold their Registry mutation lock/transaction while passing
/// the complete occupied set. The plain display-name slug is preferred; a
/// short server identity suffix is used only when two servers normalize to the
/// same slug.
pub fn allocate_model_namespace<'a>(
    display_name: &str,
    server_id: McpServerId,
    occupied: impl IntoIterator<Item = &'a McpModelNamespace>,
) -> Result<McpModelNamespace, McpError> {
    let occupied = occupied
        .into_iter()
        .map(McpModelNamespace::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let base = normalize_model_namespace_stem(display_name);
    if !occupied.contains(base.as_str()) {
        return McpModelNamespace::from_normalized(base);
    }

    let compact_id = server_id.as_uuid().simple().to_string();
    for suffix in [
        compact_id.get(..8),
        compact_id.get(..12),
        compact_id.get(..16),
    ]
    .into_iter()
    .flatten()
    {
        let candidate = namespace_with_suffix(&base, suffix);
        if !occupied.contains(candidate.as_str()) {
            return McpModelNamespace::from_normalized(candidate);
        }
    }
    for sequence in 2_u32..=10_000 {
        let suffix = format!("{}_{sequence}", &compact_id[..8]);
        let candidate = namespace_with_suffix(&base, &suffix);
        if !occupied.contains(candidate.as_str()) {
            return McpModelNamespace::from_normalized(candidate);
        }
    }
    Err(McpError::config("MCP model namespace space is exhausted"))
}

fn normalize_model_namespace_stem(display_name: &str) -> String {
    let mut output = String::new();
    let mut previous_separator = false;
    for byte in display_name.bytes() {
        if byte.is_ascii_alphanumeric() {
            if output.len() == MCP_MODEL_NAMESPACE_MAX_BYTES {
                break;
            }
            output.push(byte.to_ascii_lowercase() as char);
            previous_separator = false;
        } else if !previous_separator && !output.is_empty() {
            if output.len() == MCP_MODEL_NAMESPACE_MAX_BYTES {
                break;
            }
            output.push('_');
            previous_separator = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    if output.is_empty() {
        "server".to_string()
    } else {
        output
    }
}

fn namespace_with_suffix(base: &str, suffix: &str) -> String {
    let base_budget = MCP_MODEL_NAMESPACE_MAX_BYTES
        .saturating_sub(1)
        .saturating_sub(suffix.len())
        .max(1);
    let mut base = base[..base.len().min(base_budget)]
        .trim_end_matches('_')
        .to_string();
    if base.is_empty() {
        base.push('s');
    }
    format!("{base}_{suffix}")
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
        let model_namespace = allocate_model_namespace(
            &config.display_name,
            config.id,
            state.entries.values().map(|entry| &entry.model_namespace),
        )?;
        let revision = Self::next_revision(&mut state)?;
        let entry = McpRegistryEntry {
            config,
            model_namespace,
            config_digest: digest,
            config_epoch: McpConfigEpoch::new(),
            revision,
        };
        state.entries.insert(entry.config.id, entry.clone());
        let change = McpRegistryChange {
            revision,
            kind: McpRegistryChangeKind::Added,
            server_id: entry.config.id,
            scope: entry.config.scope.clone(),
            enabled: entry.config.enabled,
            config_digest: entry.config_digest.clone(),
            config_epoch: entry.config_epoch,
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
        let existing_namespace = state
            .entries
            .get(&config.id)
            .map(|entry| entry.model_namespace.clone());
        let existed = existing_namespace.is_some();
        let model_namespace = match existing_namespace {
            Some(namespace) => namespace,
            None => allocate_model_namespace(
                &config.display_name,
                config.id,
                state.entries.values().map(|entry| &entry.model_namespace),
            )?,
        };
        let revision = Self::next_revision(&mut state)?;
        let entry = McpRegistryEntry {
            config,
            model_namespace,
            config_digest: digest,
            config_epoch: McpConfigEpoch::new(),
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
            scope: entry.config.scope.clone(),
            enabled: entry.config.enabled,
            config_digest: entry.config_digest.clone(),
            config_epoch: entry.config_epoch,
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
            scope: removed.config.scope.clone(),
            enabled: removed.config.enabled,
            config_digest: removed.config_digest.clone(),
            config_epoch: removed.config_epoch,
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
        McpRegistrySubscription::from_sender(&self.changes)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        McpApprovalMode, McpEnvBinding, McpServerScope, McpStdioConfig, McpTransportConfig,
        McpTrustLevel,
    };

    fn config(id: McpServerId, display_name: &str) -> McpServerConfig {
        McpServerConfig {
            id,
            display_name: display_name.to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::UserApproved,
            approval_mode: McpApprovalMode::Prompt,
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
        assert_eq!(change.scope, McpServerScope::User);
    }

    #[tokio::test]
    async fn update_and_remove_changes_keep_only_safe_old_source_identity() {
        let registry = InMemoryMcpRegistry::new();
        let mut changes = registry.subscribe();
        let id = McpServerId::new();
        let original = config(id, "original");
        registry.add(original.clone()).unwrap();
        let added = changes.recv().await.unwrap();
        assert_eq!(added.kind, McpRegistryChangeKind::Added);

        let mut updated = original;
        updated.display_name = "updated".to_string();
        updated.scope = McpServerScope::Project {
            project_id: "safe-project-id".to_string(),
        };
        let expected_digest = match registry.upsert(updated).unwrap() {
            McpRegistryMutation::Updated(entry) => entry.config_digest,
            other => panic!("expected update, got {other:?}"),
        };
        let update = changes.recv().await.unwrap();
        assert_eq!(update.kind, McpRegistryChangeKind::Updated);
        assert_eq!(
            update.scope,
            McpServerScope::Project {
                project_id: "safe-project-id".to_string()
            }
        );
        assert_eq!(update.config_digest, expected_digest);

        registry.remove(id).unwrap().unwrap();
        let removed = changes.recv().await.unwrap();
        assert_eq!(removed.kind, McpRegistryChangeKind::Removed);
        assert_eq!(removed.scope, update.scope);
        assert_eq!(removed.config_digest, update.config_digest);
        let encoded = serde_json::to_string(&removed).unwrap();
        assert!(!encoded.contains("opaque-secret-reference"));
        assert!(!encoded.contains("TOKEN"));
    }

    #[test]
    fn add_rejects_duplicate_ids_and_upsert_detects_no_change() {
        let registry = InMemoryMcpRegistry::new();
        let id = McpServerId::new();
        let original = config(id, "same");
        let added = registry.add(original.clone()).unwrap();
        assert!(registry.add(original.clone()).is_err());
        let unchanged = match registry.upsert(original).unwrap() {
            McpRegistryMutation::Unchanged(entry) => entry,
            other => panic!("expected unchanged entry, got {other:?}"),
        };
        assert_eq!(unchanged.config_epoch, added.config_epoch);
        assert_eq!(unchanged.revision, added.revision);
    }

    #[test]
    fn model_namespaces_are_friendly_unique_and_stable_across_renames() {
        let registry = InMemoryMcpRegistry::new();
        let first_id = McpServerId::from_uuid(
            uuid::Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        );
        let second_id = McpServerId::from_uuid(
            uuid::Uuid::parse_str("87654321-1234-4234-8234-123456789abc").unwrap(),
        );
        let first = registry.add(config(first_id, "Filesystem Test")).unwrap();
        let second = registry.add(config(second_id, "Filesystem/Test")).unwrap();

        assert_eq!(first.model_namespace.as_str(), "filesystem_test");
        assert_eq!(second.model_namespace.as_str(), "filesystem_test_87654321");

        let mut renamed = first.config.clone();
        renamed.display_name = "A completely different label".to_string();
        let renamed = match registry.upsert(renamed).unwrap() {
            McpRegistryMutation::Updated(entry) => entry,
            other => panic!("expected updated entry, got {other:?}"),
        };
        assert_eq!(renamed.model_namespace, first.model_namespace);
    }

    #[test]
    fn model_namespace_falls_back_safely_for_non_ascii_display_names() {
        let id = McpServerId::new();
        let namespace = allocate_model_namespace("文件系统", id, std::iter::empty()).unwrap();
        assert_eq!(namespace.as_str(), "server");
        assert!(namespace.as_str().len() <= MCP_MODEL_NAMESPACE_MAX_BYTES);
    }

    #[tokio::test]
    async fn effective_mutations_never_reuse_a_configuration_epoch() {
        let registry = InMemoryMcpRegistry::new();
        let mut changes = registry.subscribe();
        let id = McpServerId::new();
        let original = config(id, "config-a");
        let added = registry.add(original.clone()).unwrap();
        let added_change = changes.recv().await.unwrap();
        assert_eq!(added_change.config_epoch, added.config_epoch);

        let mut changed = original.clone();
        changed.display_name = "config-b".to_string();
        let changed = match registry.upsert(changed).unwrap() {
            McpRegistryMutation::Updated(entry) => entry,
            other => panic!("expected updated entry, got {other:?}"),
        };
        assert_ne!(changed.config_epoch, added.config_epoch);
        assert!(changed.revision > added.revision);
        let _ = changes.recv().await.unwrap();

        let restored = match registry.upsert(original.clone()).unwrap() {
            McpRegistryMutation::Updated(entry) => entry,
            other => panic!("expected restored entry, got {other:?}"),
        };
        assert_eq!(restored.config_digest, added.config_digest);
        assert_ne!(restored.config_epoch, added.config_epoch);
        assert_ne!(restored.config_epoch, changed.config_epoch);
        assert!(restored.revision > changed.revision);
        let _ = changes.recv().await.unwrap();

        let removed = registry.remove(id).unwrap().unwrap();
        let removed_change = changes.recv().await.unwrap();
        assert_eq!(removed_change.config_epoch, restored.config_epoch);
        assert_eq!(removed_change.revision, removed.revision);

        let readded = registry.add(original).unwrap();
        assert_eq!(readded.config_digest, added.config_digest);
        assert_ne!(readded.config_epoch, removed.config_epoch);
        assert!(readded.revision > removed.revision);
    }
}
