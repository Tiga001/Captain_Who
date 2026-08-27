use super::StorageService;
use crate::storage::browser_data_repository;
use crate::storage::models::{
    BrowserHistoryDeleteInput, BrowserHistoryListInput, BrowserHistoryMetadataUpdate,
    BrowserHistoryRecord, BrowserHistoryRegistration, BrowserOwnedDataClearInput,
    BrowserOwnedDataClearOutput, BrowserOwnedDataRangeInput, BrowserOwnedDataSummary,
    BrowserPreferencesRecord, BrowserPreferencesUpdate, BROWSER_DATA_SCHEMA_VERSION,
};
use crate::storage::storage_error;
use std::collections::HashSet;
use url::Url;
use uuid::Uuid;

const HISTORY_ID_PREFIX: &str = "browser-history:";
const MAX_HISTORY_QUERY_BYTES: usize = 256;
const MAX_HISTORY_LIMIT: u32 = 500;
const MAX_HISTORY_DELETE_COUNT: usize = 500;

impl StorageService {
    pub fn load_browser_preferences(&self) -> Result<BrowserPreferencesRecord, String> {
        let connection = self.state.connection()?;
        browser_data_repository::load_preferences(&connection).map_err(storage_error)
    }

    pub fn save_browser_preferences(
        &self,
        mut update: BrowserPreferencesUpdate,
    ) -> Result<BrowserPreferencesRecord, String> {
        validate_schema(update.schema_version)?;
        let current = self.load_browser_preferences()?;
        if update.expected_revision != current.revision {
            return Err("browser.data.preferences_conflict".to_string());
        }
        if update.updated_at <= current.updated_at {
            update.updated_at = current.updated_at.saturating_add(1);
        }
        let mut connection = self.state.connection()?;
        browser_data_repository::save_preferences(&mut connection, &update)
            .map_err(storage_error)?
            .ok_or_else(|| "browser.data.preferences_conflict".to_string())
    }

    pub fn register_browser_history(
        &self,
        input: BrowserHistoryRegistration,
    ) -> Result<BrowserHistoryRecord, String> {
        validate_history_record(&input)?;
        let connection = self.state.connection()?;
        browser_data_repository::register_history(&connection, &input).map_err(storage_error)
    }

    pub fn update_browser_history_metadata(
        &self,
        input: BrowserHistoryMetadataUpdate,
    ) -> Result<bool, String> {
        validate_schema(input.schema_version)?;
        validate_history_id(&input.history_id)?;
        validate_text(&input.title, 1024)?;
        if let Some(favicon_url) = input.favicon_url.as_deref() {
            validate_http_url(favicon_url, 4096)?;
        }
        let connection = self.state.connection()?;
        browser_data_repository::update_history_metadata(&connection, &input).map_err(storage_error)
    }

    pub fn list_browser_history(
        &self,
        input: BrowserHistoryListInput,
    ) -> Result<Vec<BrowserHistoryRecord>, String> {
        validate_schema(input.schema_version)?;
        if input.query.len() > MAX_HISTORY_QUERY_BYTES
            || input.query.chars().any(char::is_control)
            || input.limit == 0
            || input.limit > MAX_HISTORY_LIMIT
        {
            return Err("browser.data.history_list_invalid".to_string());
        }
        let connection = self.state.connection()?;
        browser_data_repository::list_history(&connection, &input).map_err(storage_error)
    }

    pub fn delete_browser_history(
        &self,
        input: BrowserHistoryDeleteInput,
    ) -> Result<usize, String> {
        validate_schema(input.schema_version)?;
        if input.history_ids.is_empty()
            || input.history_ids.len() > MAX_HISTORY_DELETE_COUNT
            || input.history_ids.iter().collect::<HashSet<_>>().len() != input.history_ids.len()
        {
            return Err("browser.data.history_delete_invalid".to_string());
        }
        for history_id in &input.history_ids {
            validate_history_id(history_id)?;
        }
        let mut connection = self.state.connection()?;
        browser_data_repository::delete_history(&mut connection, &input.history_ids)
            .map_err(storage_error)
    }

    pub fn summarize_browser_owned_data(
        &self,
        input: BrowserOwnedDataRangeInput,
    ) -> Result<BrowserOwnedDataSummary, String> {
        validate_range(input.schema_version, input.since)?;
        let connection = self.state.connection()?;
        browser_data_repository::summarize_owned_data(&connection, &input).map_err(storage_error)
    }

    pub fn clear_browser_owned_data(
        &self,
        input: BrowserOwnedDataClearInput,
    ) -> Result<BrowserOwnedDataClearOutput, String> {
        validate_range(input.schema_version, input.since)?;
        let mut connection = self.state.connection()?;
        browser_data_repository::clear_owned_data(&mut connection, &input).map_err(storage_error)
    }
}

fn validate_history_record(input: &BrowserHistoryRecord) -> Result<(), String> {
    validate_schema(input.schema_version)?;
    validate_history_id(&input.history_id)?;
    let url = validate_http_url(&input.url, 8192)?;
    validate_text(&input.title, 1024)?;
    validate_text(&input.hostname, 255)?;
    if input.hostname != url.host_str().unwrap_or_default().to_lowercase() {
        return Err("browser.data.history_invalid".to_string());
    }
    if let Some(favicon_url) = input.favicon_url.as_deref() {
        validate_http_url(favicon_url, 4096)?;
    }
    if input.visited_at < 0 {
        return Err("browser.data.history_invalid".to_string());
    }
    Ok(())
}

fn validate_schema(schema_version: u32) -> Result<(), String> {
    if schema_version != BROWSER_DATA_SCHEMA_VERSION {
        return Err("browser.data.schema_unsupported".to_string());
    }
    Ok(())
}

fn validate_range(schema_version: u32, since: Option<i64>) -> Result<(), String> {
    validate_schema(schema_version)?;
    if since.is_some_and(|value| value < 0) {
        return Err("browser.data.range_invalid".to_string());
    }
    Ok(())
}

fn validate_history_id(history_id: &str) -> Result<(), String> {
    let uuid = history_id
        .strip_prefix(HISTORY_ID_PREFIX)
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| value.get_version_num() == 4)
        .filter(|value| value.to_string() == history_id.trim_start_matches(HISTORY_ID_PREFIX));
    if uuid.is_none() {
        return Err("browser.data.history_invalid".to_string());
    }
    Ok(())
}

fn validate_text(value: &str, maximum_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        return Err("browser.data.history_invalid".to_string());
    }
    Ok(())
}

fn validate_http_url(value: &str, maximum_bytes: usize) -> Result<Url, String> {
    if value.len() > maximum_bytes || value.chars().any(char::is_control) {
        return Err("browser.data.history_invalid".to_string());
    }
    let parsed = Url::parse(value).map_err(|_| "browser.data.history_invalid".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
    {
        return Err("browser.data.history_invalid".to_string());
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{
        BrowserLinkOpenTarget, BrowserOwnedDataClearInput, BrowserOwnedDataRangeInput,
        BrowserPreferencesUpdate,
    };

    fn history(id: &str, hostname: &str, visited_at: i64) -> BrowserHistoryRecord {
        BrowserHistoryRecord {
            schema_version: BROWSER_DATA_SCHEMA_VERSION,
            history_id: format!("browser-history:{id}"),
            url: format!("https://{hostname}/page?q=1"),
            title: format!("Page {hostname}"),
            hostname: hostname.to_string(),
            favicon_url: Some(format!("https://{hostname}/favicon.ico")),
            visited_at,
        }
    }

    #[test]
    fn browser_preferences_use_optimistic_concurrency() {
        let root = tempfile::tempdir().unwrap();
        let service = StorageService::open(&root.path().join("storage.sqlite")).unwrap();

        let saved = service
            .save_browser_preferences(BrowserPreferencesUpdate {
                schema_version: BROWSER_DATA_SCHEMA_VERSION,
                link_open_target: BrowserLinkOpenTarget::Builtin,
                expected_revision: 0,
                updated_at: 10,
            })
            .unwrap();
        assert_eq!(saved.link_open_target, BrowserLinkOpenTarget::Builtin);
        assert_eq!(saved.revision, 1);
        assert_eq!(
            service
                .save_browser_preferences(BrowserPreferencesUpdate {
                    schema_version: BROWSER_DATA_SCHEMA_VERSION,
                    link_open_target: BrowserLinkOpenTarget::System,
                    expected_revision: 0,
                    updated_at: 11,
                })
                .unwrap_err(),
            "browser.data.preferences_conflict"
        );
    }

    #[test]
    fn browser_history_supports_search_selection_and_time_ranges() {
        let root = tempfile::tempdir().unwrap();
        let service = StorageService::open(&root.path().join("storage.sqlite")).unwrap();
        let first = history("123e4567-e89b-42d3-a456-426614174010", "example.com", 100);
        let second = history("123e4567-e89b-42d3-a456-426614174011", "openai.com", 200);
        service.register_browser_history(first.clone()).unwrap();
        service.register_browser_history(second.clone()).unwrap();

        let searched = service
            .list_browser_history(BrowserHistoryListInput {
                schema_version: BROWSER_DATA_SCHEMA_VERSION,
                query: "openai".to_string(),
                limit: 20,
            })
            .unwrap();
        assert_eq!(searched, vec![second.clone()]);

        let summary = service
            .summarize_browser_owned_data(BrowserOwnedDataRangeInput {
                schema_version: BROWSER_DATA_SCHEMA_VERSION,
                since: Some(150),
            })
            .unwrap();
        assert_eq!(summary.history_count, 1);
        assert_eq!(summary.history_site_count, 1);

        let cleared = service
            .clear_browser_owned_data(BrowserOwnedDataClearInput {
                schema_version: BROWSER_DATA_SCHEMA_VERSION,
                since: Some(150),
                clear_history: true,
                clear_downloads: false,
            })
            .unwrap();
        assert_eq!(cleared.deleted_history_count, 1);
        assert_eq!(
            service
                .list_browser_history(BrowserHistoryListInput {
                    schema_version: BROWSER_DATA_SCHEMA_VERSION,
                    query: String::new(),
                    limit: 20,
                })
                .unwrap(),
            vec![first]
        );
    }
}
