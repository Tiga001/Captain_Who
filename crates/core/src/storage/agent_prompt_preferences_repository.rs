// Rust core storage.
use crate::storage::models::AgentPromptPreferencesRecord;
use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension};

const DEFAULT_WORK_MODE: &str = "coding";
const DEFAULT_TONE: &str = "pragmatic";
const DEFAULT_DETAIL_LEVEL: &str = "medium";
const MAX_CUSTOM_INSTRUCTIONS_CHARS: usize = 8_000;

pub fn load_agent_prompt_preferences(
    connection: &Connection,
) -> rusqlite::Result<AgentPromptPreferencesRecord> {
    let preferences = connection
        .query_row(
            "
            SELECT work_mode, tone, detail_level, custom_instructions, updated_at
            FROM agent_prompt_preferences
            WHERE id = 'default'
            ",
            [],
            |row| {
                Ok(AgentPromptPreferencesRecord {
                    work_mode: row.get(0)?,
                    tone: row.get(1)?,
                    detail_level: row.get(2)?,
                    custom_instructions: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .optional()?;

    Ok(preferences
        .map(normalize_preferences)
        .unwrap_or_else(default_agent_prompt_preferences))
}

pub fn save_agent_prompt_preferences(
    connection: &Connection,
    preferences: AgentPromptPreferencesRecord,
) -> rusqlite::Result<AgentPromptPreferencesRecord> {
    let preferences = normalize_preferences(preferences);
    let timestamp = now_ms();

    connection.execute(
        "
        INSERT INTO agent_prompt_preferences (
            id,
            work_mode,
            tone,
            detail_level,
            custom_instructions,
            updated_at
        )
        VALUES ('default', ?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(id) DO UPDATE SET
            work_mode = excluded.work_mode,
            tone = excluded.tone,
            detail_level = excluded.detail_level,
            custom_instructions = excluded.custom_instructions,
            updated_at = excluded.updated_at
        ",
        params![
            &preferences.work_mode,
            &preferences.tone,
            &preferences.detail_level,
            &preferences.custom_instructions,
            timestamp
        ],
    )?;

    Ok(AgentPromptPreferencesRecord {
        updated_at: timestamp,
        ..preferences
    })
}

fn default_agent_prompt_preferences() -> AgentPromptPreferencesRecord {
    AgentPromptPreferencesRecord {
        work_mode: DEFAULT_WORK_MODE.to_string(),
        tone: DEFAULT_TONE.to_string(),
        detail_level: DEFAULT_DETAIL_LEVEL.to_string(),
        custom_instructions: String::new(),
        updated_at: 0,
    }
}

fn normalize_preferences(
    preferences: AgentPromptPreferencesRecord,
) -> AgentPromptPreferencesRecord {
    AgentPromptPreferencesRecord {
        work_mode: normalize_value(
            &preferences.work_mode,
            &[DEFAULT_WORK_MODE, "general"],
            DEFAULT_WORK_MODE,
        ),
        tone: normalize_value(&preferences.tone, &["friendly", DEFAULT_TONE], DEFAULT_TONE),
        detail_level: normalize_value(
            &preferences.detail_level,
            &["low", DEFAULT_DETAIL_LEVEL, "high"],
            DEFAULT_DETAIL_LEVEL,
        ),
        custom_instructions: truncate_custom_instructions(&preferences.custom_instructions),
        updated_at: preferences.updated_at,
    }
}

fn normalize_value(value: &str, allowed_values: &[&str], fallback: &str) -> String {
    let value = value.trim();
    if allowed_values.contains(&value) {
        value.to_string()
    } else {
        fallback.to_string()
    }
}

fn truncate_custom_instructions(value: &str) -> String {
    let value = value.trim();
    let mut output = value
        .chars()
        .take(MAX_CUSTOM_INSTRUCTIONS_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_CUSTOM_INSTRUCTIONS_CHARS {
        output.push_str("\n...[truncated]");
    }

    output
}
