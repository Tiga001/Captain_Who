pub mod agent_action_audit_repository;
pub mod agent_prompt_preferences_repository;
pub mod attachment_repository;
pub mod chat_repository;
pub mod chat_search_repository;
pub mod composer_draft_repository;
pub mod config_repository;
pub mod context_compaction_audit_repository;
pub mod context_compaction_receipt_repository;
pub mod context_compaction_repository;
pub mod conversation_fork_repository;
pub mod conversation_history_archive_repository;
pub mod conversation_history_repository;
pub mod conversation_trace_repository;
mod database_snapshot;
pub mod file_draft_repository;
pub mod guidance_repository;
pub mod image_generation_execution_repository;
pub mod image_generation_repository;
pub mod migrations;
pub mod model_request_observation_repository;
pub mod models;
pub mod pending_action_repository;
pub mod preferences_repository;
pub mod project_repository;
pub mod service;
pub mod skill_enablement_repository;
pub mod task_state_repository;
pub mod turn_diff_repository;
pub mod usage_repository;
pub mod world_state_repository;

pub use database_snapshot::create_verified_sqlite_snapshot;

use rusqlite::Connection;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct StorageState {
    connection: Mutex<Connection>,
}

impl StorageState {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = database_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        migrations::run_migrations(&connection)?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "数据库连接状态不可用。".to_string())
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
