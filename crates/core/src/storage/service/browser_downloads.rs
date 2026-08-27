use super::StorageService;
use crate::storage::browser_download_repository;
use crate::storage::models::{
    BrowserDownloadListInput, BrowserDownloadLocationMode, BrowserDownloadRecord,
    BrowserDownloadRegistration, BrowserDownloadSettingsRecord, BrowserDownloadSettingsUpdate,
    BrowserDownloadSource, BROWSER_DOWNLOAD_SCHEMA_VERSION,
};
use crate::storage::storage_error;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const DOWNLOAD_ID_PREFIX: &str = "browser-download:";
const MAX_HISTORY_QUERY_BYTES: usize = 256;
const MAX_HISTORY_LIMIT: u32 = 500;

impl StorageService {
    pub fn load_browser_download_settings(&self) -> Result<BrowserDownloadSettingsRecord, String> {
        let connection = self.state.connection()?;
        browser_download_repository::load_settings(&connection).map_err(storage_error)
    }

    pub fn save_browser_download_settings(
        &self,
        mut update: BrowserDownloadSettingsUpdate,
    ) -> Result<BrowserDownloadSettingsRecord, String> {
        validate_schema(update.schema_version)?;
        let current = self.load_browser_download_settings()?;
        if update.expected_revision != current.revision {
            return Err("browser.download.settings_conflict".to_string());
        }
        if update.updated_at <= current.updated_at {
            update.updated_at = current.updated_at.saturating_add(1);
        }
        update.custom_directory = match update.location_mode {
            BrowserDownloadLocationMode::System => {
                if update.custom_directory.is_some() {
                    return Err("browser.download.settings_invalid".to_string());
                }
                None
            }
            BrowserDownloadLocationMode::Custom => {
                let raw = update
                    .custom_directory
                    .as_deref()
                    .ok_or_else(|| "browser.download.settings_invalid".to_string())?;
                Some(
                    validate_download_directory(raw)?
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        let mut connection = self.state.connection()?;
        browser_download_repository::save_settings(&mut connection, &update)
            .map_err(storage_error)?
            .ok_or_else(|| "browser.download.settings_conflict".to_string())
    }

    pub fn register_browser_download(
        &self,
        mut input: BrowserDownloadRegistration,
    ) -> Result<BrowserDownloadRecord, String> {
        validate_registration(&input)?;
        let path = validate_download_file(&input.absolute_path, input.size_bytes)?;
        input.absolute_path = path.to_string_lossy().into_owned();
        let mut connection = self.state.connection()?;
        browser_download_repository::register(&mut connection, &input).map_err(storage_error)
    }

    pub fn list_browser_downloads(
        &self,
        input: BrowserDownloadListInput,
    ) -> Result<Vec<BrowserDownloadRecord>, String> {
        validate_schema(input.schema_version)?;
        if input.query.len() > MAX_HISTORY_QUERY_BYTES
            || input.query.chars().any(char::is_control)
            || input.limit == 0
            || input.limit > MAX_HISTORY_LIMIT
        {
            return Err("browser.download.list_invalid".to_string());
        }
        let connection = self.state.connection()?;
        browser_download_repository::list(&connection, &input).map_err(storage_error)
    }

    pub fn load_browser_download(
        &self,
        download_id: &str,
    ) -> Result<Option<BrowserDownloadRecord>, String> {
        validate_download_id(download_id)?;
        let connection = self.state.connection()?;
        browser_download_repository::load_by_id(&connection, download_id).map_err(storage_error)
    }

    pub fn clear_browser_download_history(&self) -> Result<usize, String> {
        let connection = self.state.connection()?;
        browser_download_repository::clear(&connection).map_err(storage_error)
    }

    /// Returns a record only when the current conversation owns the Agent download, shares its
    /// project, or the caller explicitly permits one user-created manual download capability.
    pub fn authorize_browser_download_input(
        &self,
        download_id: &str,
        conversation_id: Option<&str>,
        allow_manual: bool,
    ) -> Result<Option<BrowserDownloadRecord>, String> {
        validate_download_id(download_id)?;
        let connection = self.state.connection()?;
        let Some(record) = browser_download_repository::load_by_id(&connection, download_id)
            .map_err(storage_error)?
        else {
            return Ok(None);
        };
        match record.source {
            BrowserDownloadSource::Manual => Ok(allow_manual.then_some(record)),
            BrowserDownloadSource::Agent => {
                let Some(conversation_id) = conversation_id else {
                    return Ok(None);
                };
                if record.conversation_id.as_deref() == Some(conversation_id) {
                    return Ok(Some(record));
                }
                let current_project = browser_download_repository::conversation_project_id(
                    &connection,
                    conversation_id,
                )
                .map_err(storage_error)?
                .flatten();
                Ok(
                    (record.project_id.is_some() && record.project_id == current_project)
                        .then_some(record),
                )
            }
        }
    }
}

fn validate_registration(input: &BrowserDownloadRegistration) -> Result<(), String> {
    validate_schema(input.schema_version)?;
    validate_download_id(&input.download_id)?;
    if !crate::browser_downloads::valid_display_name(&input.display_name) {
        return Err("browser.download.registration_invalid".to_string());
    }
    if !crate::browser_downloads::valid_mime_type(&input.mime_type) {
        return Err("browser.download.registration_invalid".to_string());
    }
    validate_sha256(&input.sha256)?;
    if input.created_at < 0 {
        return Err("browser.download.registration_invalid".to_string());
    }
    if let Some(origin) = input.source_origin.as_deref() {
        validate_text(origin, 2048)?;
    }
    match input.source {
        BrowserDownloadSource::Manual => {
            if input.conversation_id.is_some() || input.run_id.is_some() || input.call_id.is_some()
            {
                return Err("browser.download.registration_invalid".to_string());
            }
        }
        BrowserDownloadSource::Agent => {
            for value in [
                input.conversation_id.as_deref(),
                input.run_id.as_deref(),
                input.call_id.as_deref(),
            ] {
                validate_text(
                    value.ok_or_else(|| "browser.download.registration_invalid".to_string())?,
                    256,
                )?;
            }
        }
    }
    Ok(())
}

fn validate_schema(schema_version: u32) -> Result<(), String> {
    if schema_version != BROWSER_DOWNLOAD_SCHEMA_VERSION {
        return Err("browser.download.schema_unsupported".to_string());
    }
    Ok(())
}

fn validate_download_id(download_id: &str) -> Result<(), String> {
    let uuid = download_id
        .strip_prefix(DOWNLOAD_ID_PREFIX)
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| value.get_version_num() == 4)
        .ok_or_else(|| "browser.download.id_invalid".to_string())?;
    if format!("{DOWNLOAD_ID_PREFIX}{uuid}") != download_id {
        return Err("browser.download.id_invalid".to_string());
    }
    Ok(())
}

fn validate_download_directory(value: &str) -> Result<PathBuf, String> {
    validate_text(value, 16384)?;
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err("browser.download.destination_invalid".to_string());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "browser.download.destination_unavailable".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("browser.download.destination_invalid".to_string());
    }
    path.canonicalize()
        .map_err(|_| "browser.download.destination_unavailable".to_string())
}

fn validate_download_file(value: &str, expected_size: u64) -> Result<PathBuf, String> {
    validate_text(value, 16384)?;
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err("browser.download.file_invalid".to_string());
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "browser.download.file_unavailable".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_size {
        return Err("browser.download.file_invalid".to_string());
    }
    path.canonicalize()
        .map_err(|_| "browser.download.file_unavailable".to_string())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err("browser.download.registration_invalid".to_string());
    }
    Ok(())
}

fn validate_text(value: &str, maximum_bytes: usize) -> Result<(), String> {
    if value.is_empty() || value.len() > maximum_bytes || value.chars().any(char::is_control) {
        return Err("browser.download.registration_invalid".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::{ChatConversationRecord, ProjectRecord};
    use sha2::{Digest, Sha256};

    fn conversation(id: &str, project_id: Option<&str>) -> ChatConversationRecord {
        ChatConversationRecord {
            id: id.to_string(),
            project_id: project_id.map(ToString::to_string),
            model_id: None,
            title: id.to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        }
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn browser_download_settings_use_cas_and_store_a_canonical_directory() {
        let root = tempfile::tempdir().unwrap();
        let service = StorageService::open(&root.path().join("storage.sqlite")).unwrap();
        let selected = root.path().join("selected");
        fs::create_dir(&selected).unwrap();

        let saved = service
            .save_browser_download_settings(BrowserDownloadSettingsUpdate {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                location_mode: BrowserDownloadLocationMode::Custom,
                custom_directory: Some(selected.to_string_lossy().into_owned()),
                ask_where_to_save: true,
                expected_revision: 0,
                updated_at: 10,
            })
            .unwrap();
        assert_eq!(saved.revision, 1);
        assert!(saved.ask_where_to_save);
        assert_eq!(
            saved.custom_directory.as_deref(),
            Some(selected.canonicalize().unwrap().to_string_lossy().as_ref())
        );
        assert_eq!(
            service
                .save_browser_download_settings(BrowserDownloadSettingsUpdate {
                    schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                    location_mode: BrowserDownloadLocationMode::System,
                    custom_directory: None,
                    ask_where_to_save: false,
                    expected_revision: 0,
                    updated_at: 11,
                })
                .unwrap_err(),
            "browser.download.settings_conflict"
        );
    }

    #[test]
    fn browser_download_history_accepts_an_empty_regular_file() {
        let root = tempfile::tempdir().unwrap();
        let service = StorageService::open(&root.path().join("storage.sqlite")).unwrap();
        let path = root.path().join("empty.txt");
        fs::write(&path, []).unwrap();

        let record = service
            .register_browser_download(BrowserDownloadRegistration {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                download_id: "browser-download:123e4567-e89b-42d3-a456-426614174002".to_string(),
                source: BrowserDownloadSource::Manual,
                display_name: "empty.txt".to_string(),
                mime_type: "text/plain".to_string(),
                size_bytes: 0,
                sha256: digest(&[]),
                absolute_path: path.to_string_lossy().into_owned(),
                source_origin: None,
                conversation_id: None,
                run_id: None,
                call_id: None,
                created_at: 1,
            })
            .unwrap();

        assert_eq!(record.size_bytes, 0);
        assert_eq!(record.sha256, digest(&[]));
    }

    #[test]
    fn browser_download_history_preserves_ownership_and_clears_only_metadata() {
        let root = tempfile::tempdir().unwrap();
        let service = StorageService::open(&root.path().join("storage.sqlite")).unwrap();
        service
            .save_project(ProjectRecord {
                id: "project-a".to_string(),
                name: "Project A".to_string(),
                path: Some(root.path().to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        for record in [
            conversation("conversation-a", Some("project-a")),
            conversation("conversation-b", Some("project-a")),
            conversation("conversation-c", None),
        ] {
            service.save_conversation(record).unwrap();
        }
        let agent_bytes = b"agent download";
        let agent_path = root.path().join("agent.zip");
        fs::write(&agent_path, agent_bytes).unwrap();
        let agent_id = "browser-download:123e4567-e89b-42d3-a456-426614174000";
        service
            .register_browser_download(BrowserDownloadRegistration {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                download_id: agent_id.to_string(),
                source: BrowserDownloadSource::Agent,
                display_name: "agent.zip".to_string(),
                mime_type: "application/zip".to_string(),
                size_bytes: agent_bytes.len() as u64,
                sha256: digest(agent_bytes),
                absolute_path: agent_path.to_string_lossy().into_owned(),
                source_origin: Some("https://example.test".to_string()),
                conversation_id: Some("conversation-a".to_string()),
                run_id: Some("run-a".to_string()),
                call_id: Some("call-a".to_string()),
                created_at: 2,
            })
            .unwrap();

        assert!(service
            .authorize_browser_download_input(agent_id, Some("conversation-a"), false)
            .unwrap()
            .is_some());
        assert!(service
            .authorize_browser_download_input(agent_id, Some("conversation-b"), false)
            .unwrap()
            .is_some());
        assert!(service
            .authorize_browser_download_input(agent_id, Some("conversation-c"), false)
            .unwrap()
            .is_none());

        let manual_bytes = b"manual download";
        let manual_path = root.path().join("manual.txt");
        fs::write(&manual_path, manual_bytes).unwrap();
        let manual_id = "browser-download:123e4567-e89b-42d3-a456-426614174001";
        service
            .register_browser_download(BrowserDownloadRegistration {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                download_id: manual_id.to_string(),
                source: BrowserDownloadSource::Manual,
                display_name: "manual.txt".to_string(),
                mime_type: "text/plain".to_string(),
                size_bytes: manual_bytes.len() as u64,
                sha256: digest(manual_bytes),
                absolute_path: manual_path.to_string_lossy().into_owned(),
                source_origin: None,
                conversation_id: None,
                run_id: None,
                call_id: None,
                created_at: 3,
            })
            .unwrap();
        assert!(service
            .authorize_browser_download_input(manual_id, Some("conversation-a"), false)
            .unwrap()
            .is_none());
        assert!(service
            .authorize_browser_download_input(manual_id, Some("conversation-a"), true)
            .unwrap()
            .is_some());

        let listed = service
            .list_browser_downloads(BrowserDownloadListInput {
                schema_version: BROWSER_DOWNLOAD_SCHEMA_VERSION,
                query: "manual".to_string(),
                limit: 20,
            })
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].download_id, manual_id);
        assert_eq!(service.clear_browser_download_history().unwrap(), 2);
        assert!(service.load_browser_download(agent_id).unwrap().is_none());
        assert_eq!(fs::read(&manual_path).unwrap(), manual_bytes);
    }
}
