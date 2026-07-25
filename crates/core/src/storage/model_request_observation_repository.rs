use crate::{
    AgentApiStyle, ModelRequestObservation, ModelRequestObservationStatus, ModelRequestPurpose,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ModelRequestObservationRepositoryError {
    Database(rusqlite::Error),
    Invalid(String),
}

impl Display for ModelRequestObservationRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "本地数据库操作失败：{error}"),
            Self::Invalid(message) => write!(formatter, "模型请求观测数据无效：{message}"),
        }
    }
}

impl Error for ModelRequestObservationRepositoryError {}

impl From<rusqlite::Error> for ModelRequestObservationRepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

pub fn insert_observation(
    connection: &Connection,
    observation: &ModelRequestObservation,
) -> Result<(), ModelRequestObservationRepositoryError> {
    observation
        .validate()
        .map_err(|error| ModelRequestObservationRepositoryError::Invalid(error.to_string()))?;
    let observation_json = serde_json::to_string(observation).map_err(|error| {
        ModelRequestObservationRepositoryError::Invalid(format!("无法序列化模型请求观测：{error}"))
    })?;
    let existing = connection
        .query_row(
            "SELECT observation_json FROM model_request_observations WHERE id = ?1",
            [&observation.id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing == observation_json {
            return Ok(());
        }
        return Err(ModelRequestObservationRepositoryError::Invalid(format!(
            "观测 ID {} 已绑定其他请求数据。",
            observation.id
        )));
    }

    connection.execute(
        "INSERT INTO model_request_observations (
            id, schema_version, run_id, conversation_id, assistant_message_id,
            operation_id, request_index, purpose, model, api_style, status,
            estimated_input_tokens, normalized_actual_input_tokens,
            observation_json, started_at, completed_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
         )",
        params![
            &observation.id,
            observation.schema_version,
            &observation.run_id,
            &observation.conversation_id,
            &observation.assistant_message_id,
            &observation.operation_id,
            observation.request_index,
            observation.purpose.as_str(),
            &observation.model,
            api_style_name(observation.api_style),
            observation.status.as_str(),
            observation
                .estimate
                .as_ref()
                .map(|estimate| estimate.estimated_input_tokens),
            observation.normalized_actual_input_tokens(),
            observation_json,
            observation.started_at,
            observation.completed_at,
        ],
    )?;
    Ok(())
}

pub fn get_observation(
    connection: &Connection,
    observation_id: &str,
) -> Result<Option<ModelRequestObservation>, ModelRequestObservationRepositoryError> {
    let row = connection
        .query_row(
            "SELECT purpose, api_style, status, observation_json
             FROM model_request_observations
             WHERE id = ?1",
            [observation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    row.map(decode_observation).transpose()
}

pub fn list_observations_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ModelRequestObservation>, ModelRequestObservationRepositoryError> {
    let rows = {
        let mut statement = connection.prepare(
            "SELECT purpose, api_style, status, observation_json
             FROM model_request_observations
             WHERE conversation_id = ?1
             ORDER BY completed_at ASC, id ASC",
        )?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    rows.into_iter().map(decode_observation).collect()
}

fn decode_observation(
    row: (String, String, String, String),
) -> Result<ModelRequestObservation, ModelRequestObservationRepositoryError> {
    let (purpose, api_style, status, observation_json) = row;
    let observation: ModelRequestObservation =
        serde_json::from_str(&observation_json).map_err(|error| {
            ModelRequestObservationRepositoryError::Invalid(format!(
                "无法解析模型请求观测：{error}"
            ))
        })?;
    observation
        .validate()
        .map_err(|error| ModelRequestObservationRepositoryError::Invalid(error.to_string()))?;
    if Some(observation.purpose) != ModelRequestPurpose::from_str(&purpose)
        || api_style_name(observation.api_style) != api_style
        || Some(observation.status) != ModelRequestObservationStatus::from_str(&status)
    {
        return Err(ModelRequestObservationRepositoryError::Invalid(
            "模型请求观测索引列与 JSON 内容不一致。".to_string(),
        ));
    }
    Ok(observation)
}

fn api_style_name(api_style: AgentApiStyle) -> &'static str {
    match api_style {
        AgentApiStyle::OpenAiCompatible => "open_ai_compatible",
        AgentApiStyle::AnthropicCompatible => "anthropic_compatible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;
    use crate::{
        AgentUsage, ModelRequestActualUsage, ModelRequestUsageNormalization,
        MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
    };

    fn setup() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-1', NULL, NULL, 'Test', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, created_at, position
                 ) VALUES ('assistant-1', 'conversation-1', 'assistant', '', 'pending', 1, 0)",
                [],
            )
            .unwrap();
        connection
    }

    fn observation() -> ModelRequestObservation {
        ModelRequestObservation {
            schema_version: MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
            id: "request-1".to_string(),
            run_id: "run-1".to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
            operation_id: None,
            request_index: 1,
            purpose: ModelRequestPurpose::AgentLoop,
            model: "model-a".to_string(),
            api_style: AgentApiStyle::OpenAiCompatible,
            status: ModelRequestObservationStatus::Completed,
            tool_set: Some(
                crate::model_request_observation::ModelRequestToolSetObservation::new(
                    AgentApiStyle::OpenAiCompatible,
                    "stable-tool-set-v1:stable",
                    "dynamic-tool-set-v1:dynamic",
                    "effective-tool-set-v1:effective",
                    20,
                    2,
                ),
            ),
            estimate: None,
            actual_usage: Some(ModelRequestActualUsage {
                raw: AgentUsage {
                    input_tokens: Some(100),
                    output_tokens: Some(10),
                    output_thinking_tokens: None,
                    total_tokens: Some(110),
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                    billable_request_count: Some(1),
                },
                normalized_input_tokens: Some(100),
                normalization: ModelRequestUsageNormalization::OpenAiInputTokens,
            }),
            finish_reason: Some("stop".to_string()),
            error_code: None,
            error_message: None,
            started_at: 1,
            completed_at: 2,
        }
    }

    #[test]
    fn inserts_idempotently_but_rejects_conflicting_identity() {
        let connection = setup();
        let observation = observation();
        insert_observation(&connection, &observation).unwrap();
        insert_observation(&connection, &observation).unwrap();
        assert_eq!(
            get_observation(&connection, "request-1").unwrap(),
            Some(observation.clone())
        );

        let mut conflicting = observation;
        conflicting.model = "model-b".to_string();
        assert!(matches!(
            insert_observation(&connection, &conflicting),
            Err(ModelRequestObservationRepositoryError::Invalid(_))
        ));
    }
}
