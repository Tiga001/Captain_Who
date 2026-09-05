//! Provider-neutral human question contracts. Ownership and resume authority are Host facts;
//! model arguments contain only questions. Displaying a response as a user bubble does not change
//! its delivery route: synchronous responses settle the original tool call exactly once.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const HUMAN_INTERACTION_SCHEMA_VERSION: u32 = 1;
pub const HUMAN_INTERACTION_MAX_INPUT_BYTES: usize = 262_144;
pub const HUMAN_INTERACTION_MAX_TITLE_BYTES: usize = 8_192;
pub const HUMAN_INTERACTION_MAX_OPTION_BYTES: usize = 2_048;
pub const HUMAN_INTERACTION_MAX_ANSWER_BYTES: usize = 32_768;
pub const HUMAN_INTERACTION_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const HUMAN_INTERACTION_MAX_DISPLAY_BYTES: usize = 1_048_576;

/// Bounded, self-contained history material for one submitted synchronous question batch.
/// Kept in its original ToolResult, including when a completed turn is copied into a fork.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionResponseDisplay {
    #[serde(rename = "type")]
    pub result_type: String,
    pub schema_version: u32,
    pub request_id: String,
    pub response_id: String,
    pub answers: Vec<HumanInteractionAnswerDisplay>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HumanInteractionAnswerDisplay {
    Option {
        question_id: String,
        question: String,
        option_id: String,
        answer: String,
    },
    Text {
        question_id: String,
        question: String,
        answer: String,
    },
    Skipped {
        question_id: String,
        question: String,
        answer: String,
    },
}

impl HumanInteractionAnswerDisplay {
    pub fn question(&self) -> &str {
        match self {
            Self::Option { question, .. }
            | Self::Text { question, .. }
            | Self::Skipped { question, .. } => question,
        }
    }
}

impl HumanInteractionResponseDisplay {
    pub fn validate(&self) -> Result<(), HumanInteractionError> {
        let invalid = HumanInteractionError::invalid;
        if self.result_type != "human_interaction_response"
            || self.schema_version != HUMAN_INTERACTION_SCHEMA_VERSION
            || serde_json::to_vec(self).map_err(|_| invalid())?.len()
                > HUMAN_INTERACTION_MAX_DISPLAY_BYTES
        {
            return Err(invalid());
        }
        validate_human_interaction_id(&self.request_id)?;
        validate_human_interaction_id(&self.response_id)?;
        let mut question_ids = BTreeSet::new();
        let mut questions = Vec::with_capacity(self.answers.len());
        let mut answers = Vec::with_capacity(self.answers.len());
        for entry in &self.answers {
            let (question_id, question, options, answer) = match entry {
                HumanInteractionAnswerDisplay::Option {
                    question_id,
                    question,
                    option_id,
                    answer,
                } => {
                    validate_human_interaction_id(option_id)?;
                    if !valid_text(answer, HUMAN_INTERACTION_MAX_OPTION_BYTES) {
                        return Err(invalid());
                    }
                    (
                        question_id,
                        question,
                        Some(vec![answer.clone()]),
                        HumanInteractionAnswer::Option {
                            question_id: question_id.clone(),
                            option_id: option_id.clone(),
                        },
                    )
                }
                HumanInteractionAnswerDisplay::Text {
                    question_id,
                    question,
                    answer,
                } => {
                    if !valid_text(answer, HUMAN_INTERACTION_MAX_ANSWER_BYTES) {
                        return Err(invalid());
                    }
                    (
                        question_id,
                        question,
                        None,
                        HumanInteractionAnswer::Text {
                            question_id: question_id.clone(),
                            text: answer.clone(),
                        },
                    )
                }
                HumanInteractionAnswerDisplay::Skipped {
                    question_id,
                    question,
                    answer,
                } => {
                    if answer != "已跳过" {
                        return Err(invalid());
                    }
                    (
                        question_id,
                        question,
                        None,
                        HumanInteractionAnswer::Skipped {
                            question_id: question_id.clone(),
                        },
                    )
                }
            };
            validate_human_interaction_id(question_id)?;
            if !question_ids.insert(question_id) {
                return Err(invalid());
            }
            questions.push(HumanInteractionQuestionInput {
                title: question.clone(),
                options,
            });
            answers.push(answer);
        }
        validate_human_interaction_tool_input(&HumanInteractionToolInput { questions })?;
        if serde_json::to_vec(&answers).map_err(|_| invalid())?.len()
            > HUMAN_INTERACTION_MAX_INPUT_BYTES
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(crate) fn valid_value(value: &serde_json::Value) -> bool {
        serde_json::from_value::<Self>(value.clone())
            .is_ok_and(|display| display.validate().is_ok())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanInteractionError {
    pub code: &'static str,
    pub message: &'static str,
}

impl HumanInteractionError {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub const fn invalid() -> Self {
        Self::new("invalid_input", "The human interaction input is invalid.")
    }
}

impl std::fmt::Display for HumanInteractionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for HumanInteractionError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionSettings {
    pub enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
}

impl Default for HumanInteractionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            revision: 0,
            updated_at: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionSettingsUpdate {
    pub enabled: bool,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HumanInteractionToolInput {
    pub questions: Vec<HumanInteractionQuestionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HumanInteractionQuestionInput {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanInteractionMode {
    Sync,
    Async,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanInteractionRequestStatus {
    Open,
    Submitted,
    Ignored,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanInteractionResponseKind {
    Submitted,
    Ignored,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanInteractionDeliveryStatus {
    Pending,
    Bound,
    Applied,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HumanInteractionOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HumanInteractionQuestion {
    pub id: String,
    pub title: String,
    pub options: Option<Vec<HumanInteractionOption>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HumanInteractionAnswer {
    Option {
        question_id: String,
        option_id: String,
    },
    Text {
        question_id: String,
        text: String,
    },
    Skipped {
        question_id: String,
    },
}

impl HumanInteractionAnswer {
    pub fn question_id(&self) -> &str {
        match self {
            Self::Option { question_id, .. }
            | Self::Text { question_id, .. }
            | Self::Skipped { question_id } => question_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionResponse {
    pub response_id: String,
    pub request_id: String,
    pub submission_id: String,
    pub kind: HumanInteractionResponseKind,
    pub answers: Vec<HumanInteractionAnswer>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionDelivery {
    pub response_id: String,
    pub status: HumanInteractionDeliveryStatus,
    pub revision: u64,
    pub target_run_id: Option<String>,
    pub user_message_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionRequestSnapshot {
    pub schema_version: u32,
    /// Host creation order, independent of wall-clock precision and model-supplied content.
    pub sequence: u64,
    pub request_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub mode: HumanInteractionMode,
    pub status: HumanInteractionRequestStatus,
    pub revision: u64,
    pub policy_revision: u64,
    pub questions: Vec<HumanInteractionQuestion>,
    pub response: Option<HumanInteractionResponse>,
    pub delivery: Option<HumanInteractionDelivery>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionSubmitInput {
    pub conversation_id: String,
    pub request_id: String,
    pub expected_revision: u64,
    pub submission_id: String,
    pub answers: Vec<HumanInteractionAnswer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionIgnoreInput {
    pub conversation_id: String,
    pub request_id: String,
    pub expected_revision: u64,
    pub submission_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionListInput {
    pub conversation_id: String,
    #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionListOutput {
    pub items: Vec<HumanInteractionRequestSnapshot>,
    pub next_cursor: Option<String>,
}

pub fn validate_human_interaction_id(value: &str) -> Result<(), HumanInteractionError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(HumanInteractionError::invalid());
    }
    Ok(())
}

fn valid_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.contains('\0')
}

pub fn validate_human_interaction_tool_input(
    input: &HumanInteractionToolInput,
) -> Result<(), HumanInteractionError> {
    if input.questions.is_empty()
        || serde_json::to_vec(input)
            .map_err(|_| HumanInteractionError::invalid())?
            .len()
            > HUMAN_INTERACTION_MAX_INPUT_BYTES
    {
        return Err(HumanInteractionError::invalid());
    }
    for question in &input.questions {
        if !valid_text(&question.title, HUMAN_INTERACTION_MAX_TITLE_BYTES) {
            return Err(HumanInteractionError::invalid());
        }
        if let Some(options) = &question.options {
            let unique: BTreeSet<_> = options.iter().map(|value| value.trim()).collect();
            if options.is_empty()
                || unique.len() != options.len()
                || options
                    .iter()
                    .any(|value| !valid_text(value, HUMAN_INTERACTION_MAX_OPTION_BYTES))
            {
                return Err(HumanInteractionError::invalid());
            }
        }
    }
    Ok(())
}

/// Validates a complete batch and returns answers in the immutable question order. Paging and
/// client ordering cannot change model delivery or idempotence material.
pub fn validate_human_interaction_answers(
    questions: &[HumanInteractionQuestion],
    answers: &[HumanInteractionAnswer],
) -> Result<Vec<HumanInteractionAnswer>, HumanInteractionError> {
    if questions.is_empty()
        || answers.len() != questions.len()
        || serde_json::to_vec(answers)
            .map_err(|_| HumanInteractionError::invalid())?
            .len()
            > HUMAN_INTERACTION_MAX_INPUT_BYTES
    {
        return Err(HumanInteractionError::invalid());
    }
    let by_id: std::collections::BTreeMap<_, _> =
        answers.iter().map(|a| (a.question_id(), a)).collect();
    let question_ids: BTreeSet<_> = questions
        .iter()
        .map(|question| question.id.as_str())
        .collect();
    if by_id.len() != answers.len() || question_ids.len() != questions.len() {
        return Err(HumanInteractionError::invalid());
    }
    let mut ordered = Vec::with_capacity(questions.len());
    for question in questions {
        let answer = by_id
            .get(question.id.as_str())
            .ok_or_else(HumanInteractionError::invalid)?;
        match answer {
            HumanInteractionAnswer::Option { option_id, .. } => {
                if !question
                    .options
                    .as_ref()
                    .is_some_and(|options| options.iter().any(|option| &option.id == option_id))
                {
                    return Err(HumanInteractionError::invalid());
                }
            }
            HumanInteractionAnswer::Text { text, .. }
                if !valid_text(text, HUMAN_INTERACTION_MAX_ANSWER_BYTES) =>
            {
                return Err(HumanInteractionError::invalid());
            }
            _ => {}
        }
        ordered.push((*answer).clone());
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests;
