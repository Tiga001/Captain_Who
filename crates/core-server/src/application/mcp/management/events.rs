//! Manager-event to management-notification projection.

use super::*;

impl McpManagementService {
    pub(crate) fn project_changed_notification(
        &self,
        sequence: u64,
        event: &McpEvent,
    ) -> Option<McpChangedNotification> {
        let (kind, server_id, state) = match event {
            McpEvent::RegistryChanged {
                kind, server_id, ..
            } => (
                match kind {
                    McpRegistryChangeKind::Added => McpChangedKindDto::Added,
                    McpRegistryChangeKind::Updated => McpChangedKindDto::Updated,
                    McpRegistryChangeKind::Removed => McpChangedKindDto::Deleted,
                    _ => return None,
                },
                *server_id,
                None,
            ),
            McpEvent::ServerStateChanged {
                server_id, current, ..
            } => (
                McpChangedKindDto::StateChanged,
                *server_id,
                Some(connection_state(*current)),
            ),
            McpEvent::CatalogChanged { server_id, .. } => {
                (McpChangedKindDto::CatalogChanged, *server_id, None)
            }
            McpEvent::ServerError { server_id, .. } | McpEvent::ServerExited { server_id, .. } => (
                McpChangedKindDto::StateChanged,
                *server_id,
                Some(McpConnectionStateDto::Error),
            ),
            McpEvent::RegistryReconciliationRequired { .. } => {
                return Some(self.project_resync_required_notification(sequence));
            }
            _ => return None,
        };
        let registry_revision = match self.registry.current_revision() {
            Ok(revision) => revision,
            Err(_) => return Some(self.project_resync_required_notification(sequence)),
        };
        Some(McpChangedNotification {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            source_epoch: self.source_epoch.clone(),
            sequence,
            registry_revision,
            kind,
            server_id: Some(server_id.to_string()),
            state,
        })
    }

    pub(crate) fn project_resync_required_notification(
        &self,
        sequence: u64,
    ) -> McpChangedNotification {
        McpChangedNotification {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            source_epoch: self.source_epoch.clone(),
            sequence,
            registry_revision: self.registry.current_revision().unwrap_or_default(),
            kind: McpChangedKindDto::ResyncRequired,
            server_id: None,
            state: None,
        }
    }
}
