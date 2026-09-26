use crate::protocol::AgentPermissions;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferencesRecord {
    pub profile_avatar_data_url: Option<String>,
    pub profile_display_name: String,
    pub profile_handle: String,
    pub sidebar_conversation_sort: String,
    pub sidebar_project_sort: String,
    pub sidebar_project_order: Vec<String>,
    pub sidebar_section_order: String,
    pub native_font_smoothing: bool,
    pub show_token_usage_details: bool,
    pub show_context_window_usage: bool,
    pub translucent_sidebar: bool,
    pub translucent_sidebar_transparency: i64,
    pub full_permission_enabled: bool,
    pub custom_permission_enabled: bool,
    pub custom_permissions: AgentPermissions,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentPromptPreferencesRecord {
    #[serde(default)]
    pub context_profile: crate::AgentContextProfile,
    pub work_mode: String,
    pub tone: String,
    pub detail_level: String,
    pub custom_instructions: String,
    pub updated_at: i64,
}
