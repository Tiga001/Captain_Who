use serde::{Deserialize, Serialize};

pub const BROWSER_DOWNLOAD_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDownloadLocationMode {
    System,
    Custom,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDownloadSource {
    Manual,
    Agent,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadSettingsRecord {
    pub schema_version: u32,
    pub location_mode: BrowserDownloadLocationMode,
    pub custom_directory: Option<String>,
    pub ask_where_to_save: bool,
    pub revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadSettingsUpdate {
    pub schema_version: u32,
    pub location_mode: BrowserDownloadLocationMode,
    pub custom_directory: Option<String>,
    pub ask_where_to_save: bool,
    pub expected_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadRegistration {
    pub schema_version: u32,
    pub download_id: String,
    pub source: BrowserDownloadSource,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub absolute_path: String,
    pub source_origin: Option<String>,
    pub conversation_id: Option<String>,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadRecord {
    pub schema_version: u32,
    pub download_id: String,
    pub source: BrowserDownloadSource,
    pub display_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub absolute_path: String,
    pub source_origin: Option<String>,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserDownloadListInput {
    pub schema_version: u32,
    pub query: String,
    pub limit: u32,
}

pub const BROWSER_DATA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserLinkOpenTarget {
    System,
    Builtin,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserPreferencesRecord {
    pub schema_version: u32,
    pub link_open_target: BrowserLinkOpenTarget,
    pub revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserPreferencesUpdate {
    pub schema_version: u32,
    pub link_open_target: BrowserLinkOpenTarget,
    pub expected_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryRecord {
    pub schema_version: u32,
    pub history_id: String,
    pub url: String,
    pub title: String,
    pub hostname: String,
    pub favicon_url: Option<String>,
    pub visited_at: i64,
}

pub type BrowserHistoryRegistration = BrowserHistoryRecord;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryMetadataUpdate {
    pub schema_version: u32,
    pub history_id: String,
    pub title: String,
    pub favicon_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryListInput {
    pub schema_version: u32,
    pub query: String,
    pub limit: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserHistoryDeleteInput {
    pub schema_version: u32,
    pub history_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataRangeInput {
    pub schema_version: u32,
    pub since: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataSummary {
    pub schema_version: u32,
    pub history_count: u64,
    pub history_site_count: u64,
    pub download_count: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataClearInput {
    pub schema_version: u32,
    pub since: Option<i64>,
    pub clear_history: bool,
    pub clear_downloads: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserOwnedDataClearOutput {
    pub schema_version: u32,
    pub deleted_history_count: u64,
    pub deleted_download_count: u64,
}
