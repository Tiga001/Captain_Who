// Rust core storage.
use std::path::Path;

use crate::storage::models::{
    AgentPromptPreferencesRecord, AppDataSnapshot, ChatConversationMetaRecord,
    ChatConversationRecord, ChatMessageRecord, ChatMessageStateRecord, ComposerDraftRecord,
    ModelSettingsRecord, ProjectRecord, UiPreferencesRecord,
};
use crate::storage::{
    agent_prompt_preferences_repository, chat_repository, composer_draft_repository,
    config_repository, preferences_repository, project_repository, storage_error, StorageState,
};

pub struct StorageService {
    state: StorageState,
}

impl StorageService {
    pub fn open(database_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            state: StorageState::open(database_path)?,
        })
    }

    pub fn load_app_data(&self) -> Result<AppDataSnapshot, String> {
        let connection = self.state.connection()?;

        Ok(AppDataSnapshot {
            model_settings: config_repository::load_model_settings(&connection)
                .map_err(storage_error)?,
            projects: project_repository::list_projects(&connection).map_err(storage_error)?,
            conversations: chat_repository::list_conversations(&connection)
                .map_err(storage_error)?,
            composer_drafts: composer_draft_repository::list_composer_drafts(&connection)
                .map_err(storage_error)?,
            ui_preferences: preferences_repository::load_ui_preferences(&connection)
                .map_err(storage_error)?,
            agent_prompt_preferences:
                agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
                    .map_err(storage_error)?,
        })
    }

    pub fn load_model_settings(&self) -> Result<Option<ModelSettingsRecord>, String> {
        let connection = self.state.connection()?;
        config_repository::load_model_settings(&connection).map_err(storage_error)
    }

    pub fn save_model_settings(&self, settings: ModelSettingsRecord) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        config_repository::save_model_settings(&mut connection, settings).map_err(storage_error)
    }

    pub fn load_agent_prompt_preferences(&self) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
            .map_err(storage_error)
    }

    pub fn save_agent_prompt_preferences(
        &self,
        preferences: AgentPromptPreferencesRecord,
    ) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::save_agent_prompt_preferences(&connection, preferences)
            .map_err(storage_error)
    }

    pub fn load_projects(&self) -> Result<Vec<ProjectRecord>, String> {
        let connection = self.state.connection()?;
        project_repository::list_projects(&connection).map_err(storage_error)
    }

    pub fn save_project(&self, project: ProjectRecord) -> Result<ProjectRecord, String> {
        let connection = self.state.connection()?;
        project_repository::save_project(&connection, project.clone()).map_err(storage_error)?;
        Ok(project)
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let connection = self.state.connection()?;
        project_repository::delete_project(&connection, project_id).map_err(storage_error)
    }

    pub fn load_conversations(&self) -> Result<Vec<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        chat_repository::list_conversations(&connection).map_err(storage_error)
    }

    pub fn save_conversation(
        &self,
        conversation: ChatConversationRecord,
    ) -> Result<ChatConversationRecord, String> {
        let mut connection = self.state.connection()?;
        chat_repository::save_conversation(&mut connection, conversation.clone())
            .map_err(storage_error)?;
        Ok(conversation)
    }

    pub fn save_conversation_meta(
        &self,
        conversation: ChatConversationMetaRecord,
    ) -> Result<ChatConversationMetaRecord, String> {
        let connection = self.state.connection()?;
        chat_repository::save_conversation_meta(&connection, &conversation)
            .map_err(storage_error)?;
        Ok(conversation)
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::delete_conversation(&connection, conversation_id).map_err(storage_error)
    }

    pub fn upsert_chat_messages(
        &self,
        conversation_id: &str,
        messages: Vec<ChatMessageRecord>,
        position_offset: i64,
    ) -> Result<Vec<ChatMessageRecord>, String> {
        let mut connection = self.state.connection()?;
        chat_repository::upsert_messages(
            &mut connection,
            conversation_id,
            &messages,
            position_offset,
        )
        .map_err(storage_error)?;
        Ok(messages)
    }

    pub fn save_chat_message_state(
        &self,
        conversation_id: &str,
        message: ChatMessageStateRecord,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_state(&connection, conversation_id, &message)
            .map_err(storage_error)
    }

    pub fn load_composer_drafts(&self) -> Result<Vec<ComposerDraftRecord>, String> {
        let connection = self.state.connection()?;
        composer_draft_repository::list_composer_drafts(&connection).map_err(storage_error)
    }

    pub fn save_composer_draft(
        &self,
        draft: ComposerDraftRecord,
    ) -> Result<ComposerDraftRecord, String> {
        let connection = self.state.connection()?;
        composer_draft_repository::save_composer_draft(&connection, draft.clone())
            .map_err(storage_error)?;
        Ok(draft)
    }

    pub fn delete_composer_draft(&self, scope_id: &str) -> Result<(), String> {
        let connection = self.state.connection()?;
        composer_draft_repository::delete_composer_draft(&connection, scope_id)
            .map_err(storage_error)
    }

    pub fn load_ui_preferences(&self) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::load_ui_preferences(&connection).map_err(storage_error)
    }

    pub fn save_ui_preferences(
        &self,
        preferences: UiPreferencesRecord,
    ) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::save_ui_preferences(&connection, preferences).map_err(storage_error)
    }
}
