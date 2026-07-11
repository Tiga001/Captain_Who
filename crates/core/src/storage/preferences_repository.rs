use crate::protocol::{
    AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
    AgentWritePermission,
};
use crate::storage::models::UiPreferencesRecord;
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

const DEFAULT_CONVERSATION_SORT: &str = "updated";
const DEFAULT_PROJECT_SORT: &str = "created";
const DEFAULT_SECTION_ORDER: &str = "projects_first";
const DEFAULT_PROFILE_DISPLAY_NAME: &str = "";
const DEFAULT_PROFILE_HANDLE: &str = "USER";
const DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY: i64 = 54;
const MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY: i64 = 50;
const MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY: i64 = 100;

pub fn load_ui_preferences(connection: &Connection) -> rusqlite::Result<UiPreferencesRecord> {
    let preferences = connection
        .query_row(
            "
            SELECT
                sidebar_conversation_sort,
                sidebar_project_sort,
                sidebar_project_order_json,
                sidebar_section_order,
                translucent_sidebar,
                translucent_sidebar_transparency,
                native_font_smoothing,
                show_token_usage_details,
                profile_display_name,
                profile_handle,
                profile_avatar_data_url,
                custom_read_permission,
                custom_write_permission,
                custom_command_permission,
                custom_patch_permission,
                full_permission_enabled,
                custom_permission_enabled,
                updated_at
            FROM ui_preferences
            WHERE id = 'default'
            ",
            [],
            |row| {
                Ok(UiPreferencesRecord {
                    sidebar_conversation_sort: row.get(0)?,
                    sidebar_project_sort: row.get(1)?,
                    sidebar_project_order: parse_project_order_json(
                        row.get::<_, String>(2)?.as_str(),
                    ),
                    sidebar_section_order: row.get(3)?,
                    translucent_sidebar: row.get::<_, i64>(4)? != 0,
                    translucent_sidebar_transparency: row.get(5)?,
                    native_font_smoothing: row.get::<_, i64>(6)? != 0,
                    show_token_usage_details: row.get::<_, i64>(7)? != 0,
                    profile_display_name: row.get(8)?,
                    profile_handle: row.get(9)?,
                    profile_avatar_data_url: row.get(10)?,
                    custom_permissions: AgentPermissions {
                        read: parse_read_permission(row.get::<_, String>(11)?.as_str()),
                        write: parse_write_permission(row.get::<_, String>(12)?.as_str()),
                        command: parse_command_permission(row.get::<_, String>(13)?.as_str()),
                        patch: parse_patch_permission(row.get::<_, String>(14)?.as_str()),
                    },
                    full_permission_enabled: row.get::<_, i64>(15)? != 0,
                    custom_permission_enabled: row.get::<_, i64>(16)? != 0,
                    updated_at: row.get(17)?,
                })
            },
        )
        .optional()?;

    Ok(preferences
        .map(normalize_preferences)
        .unwrap_or_else(default_ui_preferences))
}

pub fn save_ui_preferences(
    connection: &Connection,
    preferences: UiPreferencesRecord,
) -> rusqlite::Result<UiPreferencesRecord> {
    let preferences = normalize_preferences(preferences);
    let timestamp = now_ms();
    let project_order_json = serde_json::to_string(&preferences.sidebar_project_order)
        .unwrap_or_else(|_| "[]".to_string());

    connection.execute(
        "
        INSERT INTO ui_preferences (
            id,
            sidebar_conversation_sort,
            sidebar_project_sort,
            sidebar_project_order_json,
            sidebar_section_order,
            translucent_sidebar,
            translucent_sidebar_transparency,
            native_font_smoothing,
            show_token_usage_details,
            profile_display_name,
            profile_handle,
            profile_avatar_data_url,
            custom_read_permission,
            custom_write_permission,
            custom_command_permission,
            custom_patch_permission,
            full_permission_enabled,
            custom_permission_enabled,
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
        ON CONFLICT(id) DO UPDATE SET
            sidebar_conversation_sort = excluded.sidebar_conversation_sort,
            sidebar_project_sort = excluded.sidebar_project_sort,
            sidebar_project_order_json = excluded.sidebar_project_order_json,
            sidebar_section_order = excluded.sidebar_section_order,
            translucent_sidebar = excluded.translucent_sidebar,
            translucent_sidebar_transparency = excluded.translucent_sidebar_transparency,
            native_font_smoothing = excluded.native_font_smoothing,
            show_token_usage_details = excluded.show_token_usage_details,
            profile_display_name = excluded.profile_display_name,
            profile_handle = excluded.profile_handle,
            profile_avatar_data_url = excluded.profile_avatar_data_url,
            custom_read_permission = excluded.custom_read_permission,
            custom_write_permission = excluded.custom_write_permission,
            custom_command_permission = excluded.custom_command_permission,
            custom_patch_permission = excluded.custom_patch_permission,
            full_permission_enabled = excluded.full_permission_enabled,
            custom_permission_enabled = excluded.custom_permission_enabled,
            updated_at = excluded.updated_at
        ",
        params![
            &preferences.sidebar_conversation_sort,
            &preferences.sidebar_project_sort,
            &project_order_json,
            &preferences.sidebar_section_order,
            if preferences.translucent_sidebar {
                1
            } else {
                0
            },
            preferences.translucent_sidebar_transparency,
            if preferences.native_font_smoothing {
                1
            } else {
                0
            },
            if preferences.show_token_usage_details {
                1
            } else {
                0
            },
            &preferences.profile_display_name,
            &preferences.profile_handle,
            preferences.profile_avatar_data_url.as_deref(),
            read_permission_value(preferences.custom_permissions.read),
            write_permission_value(preferences.custom_permissions.write),
            command_permission_value(preferences.custom_permissions.command),
            patch_permission_value(preferences.custom_permissions.patch),
            if preferences.full_permission_enabled { 1 } else { 0 },
            if preferences.custom_permission_enabled { 1 } else { 0 },
            timestamp,
        ],
    )?;

    Ok(UiPreferencesRecord {
        updated_at: timestamp,
        ..preferences
    })
}

fn default_ui_preferences() -> UiPreferencesRecord {
    UiPreferencesRecord {
        profile_avatar_data_url: None,
        profile_display_name: DEFAULT_PROFILE_DISPLAY_NAME.to_string(),
        profile_handle: DEFAULT_PROFILE_HANDLE.to_string(),
        sidebar_conversation_sort: DEFAULT_CONVERSATION_SORT.to_string(),
        sidebar_project_sort: DEFAULT_PROJECT_SORT.to_string(),
        sidebar_project_order: Vec::new(),
        sidebar_section_order: DEFAULT_SECTION_ORDER.to_string(),
        native_font_smoothing: false,
        show_token_usage_details: true,
        translucent_sidebar: false,
        translucent_sidebar_transparency: DEFAULT_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
        full_permission_enabled: true,
        custom_permission_enabled: true,
        custom_permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            patch: AgentPatchPermission::RequireApproval,
        },
        updated_at: 0,
    }
}

fn normalize_preferences(preferences: UiPreferencesRecord) -> UiPreferencesRecord {
    UiPreferencesRecord {
        profile_avatar_data_url: normalize_avatar_data_url(preferences.profile_avatar_data_url),
        profile_display_name: normalize_text(
            &preferences.profile_display_name,
            DEFAULT_PROFILE_DISPLAY_NAME,
        ),
        profile_handle: normalize_profile_handle(&preferences.profile_handle),
        sidebar_conversation_sort: normalize_value(
            &preferences.sidebar_conversation_sort,
            &[DEFAULT_CONVERSATION_SORT, "created"],
            DEFAULT_CONVERSATION_SORT,
        ),
        sidebar_project_sort: normalize_value(
            &preferences.sidebar_project_sort,
            &[DEFAULT_PROJECT_SORT, "recent", "manual"],
            DEFAULT_PROJECT_SORT,
        ),
        sidebar_project_order: normalize_project_order(preferences.sidebar_project_order),
        sidebar_section_order: normalize_value(
            &preferences.sidebar_section_order,
            &[DEFAULT_SECTION_ORDER, "conversations_first"],
            DEFAULT_SECTION_ORDER,
        ),
        native_font_smoothing: preferences.native_font_smoothing,
        show_token_usage_details: preferences.show_token_usage_details,
        translucent_sidebar: preferences.translucent_sidebar,
        translucent_sidebar_transparency: preferences.translucent_sidebar_transparency.clamp(
            MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
            MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
        ),
        full_permission_enabled: preferences.full_permission_enabled,
        custom_permission_enabled: preferences.custom_permission_enabled,
        custom_permissions: preferences.custom_permissions,
        updated_at: preferences.updated_at,
    }
}

fn parse_read_permission(value: &str) -> AgentReadPermission {
    match value {
        "all" => AgentReadPermission::All,
        _ => AgentReadPermission::WorkspaceOnly,
    }
}

fn parse_write_permission(value: &str) -> AgentWritePermission {
    match value {
        "denied" => AgentWritePermission::Denied,
        "all" => AgentWritePermission::All,
        _ => AgentWritePermission::WorkspaceOnly,
    }
}

fn parse_command_permission(value: &str) -> AgentCommandPermission {
    match value {
        "auto_approve" => AgentCommandPermission::AutoApprove,
        _ => AgentCommandPermission::RequireApproval,
    }
}

fn parse_patch_permission(value: &str) -> AgentPatchPermission {
    match value {
        "auto_approve" => AgentPatchPermission::AutoApprove,
        _ => AgentPatchPermission::RequireApproval,
    }
}

fn read_permission_value(value: AgentReadPermission) -> &'static str {
    match value {
        AgentReadPermission::WorkspaceOnly => "workspace_only",
        AgentReadPermission::All => "all",
    }
}

fn write_permission_value(value: AgentWritePermission) -> &'static str {
    match value {
        AgentWritePermission::Denied => "denied",
        AgentWritePermission::WorkspaceOnly => "workspace_only",
        AgentWritePermission::All => "all",
    }
}

fn command_permission_value(value: AgentCommandPermission) -> &'static str {
    match value {
        AgentCommandPermission::RequireApproval => "require_approval",
        AgentCommandPermission::AutoApprove => "auto_approve",
    }
}

fn patch_permission_value(value: AgentPatchPermission) -> &'static str {
    match value {
        AgentPatchPermission::RequireApproval => "require_approval",
        AgentPatchPermission::AutoApprove => "auto_approve",
    }
}

fn normalize_avatar_data_url(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    if value.starts_with("data:image/") {
        Some(value)
    } else {
        None
    }
}

fn normalize_text(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn normalize_profile_handle(value: &str) -> String {
    let value = value.trim().trim_start_matches('@');
    if value.is_empty() {
        DEFAULT_PROFILE_HANDLE.to_string()
    } else {
        value.to_string()
    }
}

fn parse_project_order_json(project_order_json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(project_order_json).unwrap_or_default()
}

fn normalize_project_order(project_order: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    project_order
        .into_iter()
        .filter_map(|project_id| {
            let project_id = project_id.trim().to_string();
            if project_id.is_empty() || !seen.insert(project_id.clone()) {
                return None;
            }
            Some(project_id)
        })
        .collect()
}

fn normalize_value(value: &str, allowed_values: &[&str], fallback: &str) -> String {
    let value = value.trim();
    if allowed_values.contains(&value) {
        value.to_string()
    } else {
        fallback.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    #[test]
    fn custom_permissions_use_expected_defaults_and_round_trip() {
        let connection = Connection::open_in_memory().expect("test database should open");
        migrations::run_migrations(&connection).expect("test database should migrate");

        let mut preferences = load_ui_preferences(&connection).expect("preferences should load");
        assert!(preferences.full_permission_enabled);
        assert!(preferences.custom_permission_enabled);
        assert_eq!(
            preferences.custom_permissions,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                patch: AgentPatchPermission::RequireApproval,
            }
        );

        preferences.custom_permissions = AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::Denied,
            command: AgentCommandPermission::AutoApprove,
            patch: AgentPatchPermission::AutoApprove,
        };
        preferences.full_permission_enabled = false;
        preferences.custom_permission_enabled = false;
        save_ui_preferences(&connection, preferences).expect("preferences should save");

        let reloaded = load_ui_preferences(&connection).expect("preferences should reload");
        assert!(!reloaded.full_permission_enabled);
        assert!(!reloaded.custom_permission_enabled);
        assert_eq!(
            reloaded.custom_permissions,
            AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::Denied,
                command: AgentCommandPermission::AutoApprove,
                patch: AgentPatchPermission::AutoApprove,
            }
        );
    }
}
