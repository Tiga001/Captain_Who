// Rust core storage.
pub mod agent_action_audit_repository;
pub mod agent_prompt_preferences_repository;
pub mod attachment_repository;
pub mod chat_repository;
pub mod chat_search_repository;
pub mod composer_draft_repository;
pub mod config_repository;
pub mod file_draft_repository;
pub mod migrations;
pub mod models;
pub mod pending_action_repository;
pub mod preferences_repository;
pub mod project_repository;
pub mod service;
pub mod usage_repository;

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
