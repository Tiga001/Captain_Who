pub mod agent_action_audit_repository;
pub mod agent_collaboration_event_repository;
pub(crate) mod agent_collaboration_run_policy_repository;
pub(crate) mod agent_collaboration_settings_repository;
pub mod agent_command_session_repository;
pub(crate) mod agent_context_profile_repository;
pub mod agent_delivery_repository;
pub mod agent_graph_repository;
pub(crate) mod agent_message_model_projection;
pub mod agent_prompt_preferences_repository;
pub mod agent_template_repository;
pub(crate) mod agent_tree_resource_scope;
pub mod attachment_repository;
pub mod automation_repository;
pub mod browser_data_repository;
pub mod browser_download_repository;
pub mod chat_repository;
pub mod chat_search_repository;
pub(crate) mod child_context_snapshot_repository;
pub(crate) mod command_session_receipt_payload;
pub mod composer_draft_repository;
pub mod config_repository;
pub mod context_compaction_receipt_repository;
pub mod context_compaction_repository;
pub(crate) mod conversation_context_adaptation_repository;
pub mod conversation_fork_repository;
pub mod conversation_history_archive_repository;
pub(crate) mod conversation_history_open;
pub mod conversation_history_repository;
pub mod conversation_model_context_repository;
pub mod conversation_trace_repository;
pub mod conversation_turn_rewrite_repository;
pub mod database_instance_lock;
mod database_snapshot;
pub mod file_change_repository;
pub mod file_change_run_grant_repository;
pub mod guidance_repository;
pub mod human_interaction_repository;
pub mod image_generation_execution_repository;
pub mod image_generation_repository;
pub mod managed_artifact_repository;
pub mod manual_context_compaction_repository;
pub mod mcp_approval_envelope_repository;
pub mod migrations;
pub mod model_request_observation_repository;
pub mod models;
pub mod notification_repository;
pub mod pending_action_repository;
pub mod preferences_repository;
pub mod project_repository;
pub(crate) mod provider_continuation_repository;
pub mod provider_transition_repository;
pub mod service;
pub mod skill_enablement_repository;
pub mod turn_diff_repository;
pub mod usage_repository;
pub mod world_state_repository;

pub use database_instance_lock::acquire_database_instance_lock;
pub use database_snapshot::create_verified_sqlite_snapshot;
pub use provider_transition_repository::{
    ProviderTransitionCompatibleCommitOutcome, ProviderTransitionTerminalRecord,
};

use rusqlite::{hooks::Action, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

/// Durable event journals remain the source of truth. These process-local signals only remove
/// fixed-rate polling latency and database traffic for commits made through this connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageEventStream {
    AgentCollaboration,
    Automation,
    Notification,
}

#[derive(Clone)]
pub struct StorageEventNotifications {
    inner: Arc<StorageEventNotificationSenders>,
}

struct StorageEventNotificationSenders {
    agent_collaboration: watch::Sender<u64>,
    automation: watch::Sender<u64>,
    notification: watch::Sender<u64>,
}

impl Default for StorageEventNotifications {
    fn default() -> Self {
        let (agent_collaboration, _) = watch::channel(0);
        let (automation, _) = watch::channel(0);
        let (notification, _) = watch::channel(0);
        Self {
            inner: Arc::new(StorageEventNotificationSenders {
                agent_collaboration,
                automation,
                notification,
            }),
        }
    }
}

impl StorageEventNotifications {
    pub fn subscribe(&self, stream: StorageEventStream) -> watch::Receiver<u64> {
        self.sender(stream).subscribe()
    }

    fn notify_table_inserted(&self, table: &str) {
        let stream = match table {
            "agent_collaboration_events" => StorageEventStream::AgentCollaboration,
            "automation_events" => StorageEventStream::Automation,
            "notification_change_events" => StorageEventStream::Notification,
            _ => return,
        };
        self.sender(stream)
            .send_modify(|sequence| *sequence = sequence.saturating_add(1));
    }

    fn sender(&self, stream: StorageEventStream) -> &watch::Sender<u64> {
        match stream {
            StorageEventStream::AgentCollaboration => &self.inner.agent_collaboration,
            StorageEventStream::Automation => &self.inner.automation,
            StorageEventStream::Notification => &self.inner.notification,
        }
    }
}

pub struct StorageState {
    connection: Mutex<Connection>,
    event_notifications: StorageEventNotifications,
}

impl StorageState {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        migrations::run_migrations(&connection)?;

        let event_notifications = StorageEventNotifications::default();
        let hook_notifications = event_notifications.clone();
        connection.update_hook(Some(
            move |action: Action, _database: &str, table: &str, _row_id: i64| {
                if action == Action::SQLITE_INSERT {
                    hook_notifications.notify_table_inserted(table);
                }
            },
        ));

        Ok(Self {
            connection: Mutex::new(connection),
            event_notifications,
        })
    }

    pub fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "数据库连接状态不可用。".to_string())
    }

    pub fn event_notifications(&self) -> StorageEventNotifications {
        self.event_notifications.clone()
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub fn storage_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}
