//! Provider-neutral contracts for the six model-facing Agent collaboration tools.
//!
//! Core owns argument/result shapes and the narrow Host capability. It deliberately has no
//! knowledge of Graph storage, dispatchers, transports, or renderer state.

use crate::provider_profile::ReasoningEffort;
use crate::{
    AgentCancellationToken, AgentDisplayStatus, AgentForkTurns, AgentSteerInputQueue,
    AgentWaitTargetSnapshot, ModelCapabilities,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub const AGENT_COLLABORATION_TOOL_NAMES: [&str; 6] = [
    "spawn_agent",
    "send_message",
    "followup_task",
    "wait_agent",
    "list_agents",
    "interrupt_agent",
];

pub const AGENT_COLLABORATION_DEFAULT_WAIT_MS: u64 = 30_000;
pub const AGENT_COLLABORATION_MAX_WAIT_MS: u64 = 300_000;
pub const AGENT_COLLABORATION_MAX_WAIT_TARGETS: usize = 32;
pub const AGENT_COLLABORATION_MAX_SELECTOR_ITEMS: usize = 32;
pub const AGENT_COLLABORATION_MAX_SELECTOR_DIRECTORY_BYTES: usize = 16 * 1024;
pub const AGENT_COLLABORATION_MAX_AGENT_ID_BYTES: usize = 128;
pub const AGENT_COLLABORATION_MAX_AGENT_TYPE_BYTES: usize = 64;
pub const AGENT_COLLABORATION_MAX_MODEL_CONFIG_ID_BYTES: usize = 512;

/// Host-authenticated identity for one tool-capable Turn. Unlike
/// [`crate::AgentCollaborationIdentity`], this also represents the root Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCollaborationCaller {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub parent_agent_id: Option<String>,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub task_name: String,
    pub task_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationTemplateSelector {
    pub agent_type: String,
    pub name: String,
    pub description: String,
    pub model_display_name: String,
    /// Capabilities of the exact default model resolved for this template when the Turn began.
    pub default_model_capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationModelSelector {
    pub model_config_id: String,
    pub display_name: String,
    /// Host-authored capabilities for this exact model configuration in this Turn's snapshot.
    pub capabilities: ModelCapabilities,
}

/// Bounded, presentation-safe selector snapshot supplied to the model for this Turn only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationSelectorDirectory {
    pub templates: Vec<AgentCollaborationTemplateSelector>,
    pub models: Vec<AgentCollaborationModelSelector>,
    pub truncated: bool,
}

impl AgentCollaborationSelectorDirectory {
    /// Sorts and bounds an untrusted settings/template projection without ever admitting secret
    /// connection fields into the directory type.
    pub fn bounded(
        mut templates: Vec<AgentCollaborationTemplateSelector>,
        mut models: Vec<AgentCollaborationModelSelector>,
    ) -> Self {
        templates.sort_by(|left, right| left.agent_type.cmp(&right.agent_type));
        models.sort_by(|left, right| left.model_config_id.cmp(&right.model_config_id));
        let original_template_count = templates.len();
        let original_model_count = models.len();
        templates.truncate(AGENT_COLLABORATION_MAX_SELECTOR_ITEMS);
        models.truncate(AGENT_COLLABORATION_MAX_SELECTOR_ITEMS);
        let mut directory = Self {
            templates,
            models,
            truncated: original_template_count > AGENT_COLLABORATION_MAX_SELECTOR_ITEMS
                || original_model_count > AGENT_COLLABORATION_MAX_SELECTOR_ITEMS,
        };
        while directory.prompt_data_json().len() > AGENT_COLLABORATION_MAX_SELECTOR_DIRECTORY_BYTES
        {
            directory.truncated = true;
            if directory.templates.len() >= directory.models.len()
                && !directory.templates.is_empty()
            {
                directory.templates.pop();
            } else if !directory.models.is_empty() {
                directory.models.pop();
            } else {
                break;
            }
        }
        directory
    }

    /// Encodes user-editable metadata as JSON data which cannot close the surrounding prompt tag
    /// or open a Markdown code boundary. `bounded` measures this exact representation, so every
    /// selector authorized for this Turn is also visible to the model.
    pub fn prompt_data_json(&self) -> String {
        serde_json::to_string(self)
            .map(|json| {
                json.replace('&', "\\u0026")
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
                    .replace('`', "\\u0060")
                    .replace('\u{2028}', "\\u2028")
                    .replace('\u{2029}', "\\u2029")
            })
            .unwrap_or_else(|_| "{\"templates\":[],\"models\":[],\"truncated\":true}".to_string())
    }

    pub fn validate(&self) -> crate::AgentResult<()> {
        let templates_are_canonical = self.templates.len()
            <= AGENT_COLLABORATION_MAX_SELECTOR_ITEMS
            && self
                .templates
                .windows(2)
                .all(|pair| pair[0].agent_type.as_str() < pair[1].agent_type.as_str())
            && self.templates.iter().all(|selector| {
                !selector.agent_type.is_empty()
                    && selector.agent_type.trim() == selector.agent_type
                    && selector.agent_type.len() <= AGENT_COLLABORATION_MAX_AGENT_TYPE_BYTES
            });
        let models_are_canonical = self.models.len() <= AGENT_COLLABORATION_MAX_SELECTOR_ITEMS
            && self
                .models
                .windows(2)
                .all(|pair| pair[0].model_config_id.as_str() < pair[1].model_config_id.as_str())
            && self.models.iter().all(|selector| {
                !selector.model_config_id.is_empty()
                    && selector.model_config_id.trim() == selector.model_config_id
                    && selector.model_config_id.len()
                        <= AGENT_COLLABORATION_MAX_MODEL_CONFIG_ID_BYTES
            });
        if !templates_are_canonical
            || !models_are_canonical
            || self.prompt_data_json().len() > AGENT_COLLABORATION_MAX_SELECTOR_DIRECTORY_BYTES
        {
            return Err(crate::AgentError::new(
                "Agent collaboration selector snapshot is not canonical or bounded.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationRunSnapshot {
    pub selector_directory: AgentCollaborationSelectorDirectory,
    pub admitted_wait_model_batches: Vec<u64>,
}

impl AgentCollaborationRunSnapshot {
    pub fn validate(&self) -> crate::AgentResult<()> {
        self.selector_directory.validate()?;
        if self.admitted_wait_model_batches.len() > 1_024
            || self
                .admitted_wait_model_batches
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.admitted_wait_model_batches.contains(&0)
        {
            return Err(crate::AgentError::new(
                "Agent collaboration wait admission snapshot is invalid.",
            ));
        }
        Ok(())
    }
}

/// Exact allow-list frozen from the bounded, model-visible selector directory for one Turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCollaborationSelectorAuthorization {
    agent_types: Vec<(String, ModelCapabilities)>,
    model_config_ids: Vec<(String, ModelCapabilities)>,
}

impl AgentCollaborationSelectorAuthorization {
    pub fn from_directory(directory: &AgentCollaborationSelectorDirectory) -> Self {
        Self {
            agent_types: directory
                .templates
                .iter()
                .map(|selector| {
                    (
                        selector.agent_type.clone(),
                        selector.default_model_capabilities,
                    )
                })
                .collect(),
            model_config_ids: directory
                .models
                .iter()
                .map(|selector| (selector.model_config_id.clone(), selector.capabilities))
                .collect(),
        }
    }

    pub fn allows_agent_type(&self, agent_type: &str) -> bool {
        self.agent_types
            .iter()
            .any(|(allowed, _)| allowed == agent_type)
    }

    pub fn allows_model_config_id(&self, model_config_id: &str) -> bool {
        self.model_config_ids
            .iter()
            .any(|(allowed, _)| allowed == model_config_id)
    }

    /// Returns the capability fact the model saw for the selector that determines this spawn's
    /// model. An explicit model overrides a template default, matching child model selection.
    pub fn expected_model_capabilities(
        &self,
        agent_type: Option<&str>,
        model_config_id: Option<&str>,
    ) -> Option<ModelCapabilities> {
        if let Some(model_config_id) = model_config_id {
            return self
                .model_config_ids
                .iter()
                .find(|(allowed, _)| allowed == model_config_id)
                .map(|(_, capabilities)| *capabilities);
        }
        agent_type.and_then(|agent_type| {
            self.agent_types
                .iter()
                .find(|(allowed, _)| allowed == agent_type)
                .map(|(_, capabilities)| *capabilities)
        })
    }
}

#[derive(Clone)]
pub struct AgentCollaborationRuntimeServices {
    pub executor: Arc<dyn AgentCollaborationExecutor>,
    pub caller: AgentCollaborationCaller,
    pub selector_directory: AgentCollaborationSelectorDirectory,
    selector_authorization: AgentCollaborationSelectorAuthorization,
    admitted_wait_model_batches: Arc<std::sync::Mutex<BTreeSet<u64>>>,
}

impl AgentCollaborationRuntimeServices {
    pub fn new(
        executor: Arc<dyn AgentCollaborationExecutor>,
        caller: AgentCollaborationCaller,
        selector_directory: AgentCollaborationSelectorDirectory,
    ) -> Self {
        let selector_authorization =
            AgentCollaborationSelectorAuthorization::from_directory(&selector_directory);
        Self {
            executor,
            caller,
            selector_directory,
            selector_authorization,
            admitted_wait_model_batches: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
        }
    }

    pub fn selector_authorization(&self) -> AgentCollaborationSelectorAuthorization {
        self.selector_authorization.clone()
    }

    pub fn try_admit_wait_model_batch(&self, model_batch_index: u64) -> crate::AgentResult<bool> {
        if model_batch_index == 0 {
            return Err(crate::AgentError::new(
                "Agent collaboration wait is missing its model batch identity.",
            ));
        }
        Ok(self
            .admitted_wait_model_batches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(model_batch_index))
    }

    pub fn run_snapshot(&self) -> AgentCollaborationRunSnapshot {
        AgentCollaborationRunSnapshot {
            selector_directory: self.selector_directory.clone(),
            admitted_wait_model_batches: self
                .admitted_wait_model_batches
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .copied()
                .collect(),
        }
    }

    pub fn with_run_snapshot(
        mut self,
        snapshot: &AgentCollaborationRunSnapshot,
    ) -> crate::AgentResult<Self> {
        snapshot.validate()?;
        self.selector_directory = snapshot.selector_directory.clone();
        self.selector_authorization =
            AgentCollaborationSelectorAuthorization::from_directory(&self.selector_directory);
        self.admitted_wait_model_batches = Arc::new(std::sync::Mutex::new(
            snapshot
                .admitted_wait_model_batches
                .iter()
                .copied()
                .collect(),
        ));
        Ok(self)
    }
}

#[derive(Debug, Clone)]
pub struct AgentCollaborationInvocation {
    pub caller: AgentCollaborationCaller,
    pub selector_authorization: AgentCollaborationSelectorAuthorization,
    /// Capability of the caller model frozen for this logical Run. It is Host-only fallback
    /// authorization for selector-less spawn and never comes from model-authored arguments.
    pub caller_model_capabilities: ModelCapabilities,
    /// Exact authority copied from the Host-authenticated ToolExecutionContext. It is not part of
    /// any model tool schema or action payload.
    pub effective_permissions: crate::AgentPermissions,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
    pub tool_call_id: String,
    pub action: AgentCollaborationAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCollaborationAction {
    Spawn(AgentSpawnRequest),
    SendMessage(AgentMessageRequest),
    FollowupTask(AgentMessageRequest),
    Wait(AgentWaitRequest),
    List,
    Interrupt { target_agent_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawnRequest {
    pub task_name: String,
    pub message: String,
    pub agent_type: Option<String>,
    pub model_config_id: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub fork_turns: AgentForkTurns,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessageRequest {
    pub target_agent_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWaitRequest {
    pub target_agent_ids: Vec<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone)]
pub struct AgentCollaborationExecutionControl {
    cancellation: AgentCancellationToken,
    steer: Option<AgentSteerInputQueue>,
}

impl AgentCollaborationExecutionControl {
    pub fn new(cancellation: AgentCancellationToken, steer: Option<AgentSteerInputQueue>) -> Self {
        Self {
            cancellation,
            steer,
        }
    }

    pub fn cancellation(&self) -> AgentCancellationToken {
        self.cancellation.clone()
    }

    pub fn steer(&self) -> Option<AgentSteerInputQueue> {
        self.steer.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCollaborationDeliveryState {
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentInterruptStatus {
    InterruptRequested,
    NoActiveTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHarnessSummary {
    pub agent_id: String,
    pub task_name: String,
    pub task_path: String,
    pub parent_agent_id: Option<String>,
    pub display_status: AgentDisplayStatus,
    pub model_display_name: Option<String>,
    pub latest_activity_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentCollaborationToolResult {
    Spawned {
        child_agent_id: String,
        task_path: String,
        model_display_name: String,
        model_capabilities: ModelCapabilities,
        status: AgentDisplayStatus,
    },
    MessageQueued {
        message_id: String,
        delivery_state: AgentCollaborationDeliveryState,
    },
    WaitReady {
        receipt_id: String,
        source_receipt_id: Option<String>,
        targets: Vec<AgentWaitTargetSnapshot>,
    },
    WaitStopped {
        reason: String,
    },
    Agents {
        agents: Vec<AgentHarnessSummary>,
    },
    Interrupted {
        target_agent_id: String,
        status: AgentInterruptStatus,
    },
}

impl AgentCollaborationToolResult {
    /// Canonical model-facing payload. The wait shape is deliberately byte-structurally equal to
    /// the durable payload committed by the delivery repository.
    pub fn into_model_value(self) -> crate::AgentResult<serde_json::Value> {
        use serde_json::json;
        Ok(match self {
            Self::Spawned {
                child_agent_id,
                task_path,
                model_display_name,
                model_capabilities,
                status,
            } => json!({
                "childAgentId": child_agent_id,
                "taskPath": task_path,
                "modelDisplayName": model_display_name,
                "modelCapabilities": model_capabilities,
                "status": status,
            }),
            Self::MessageQueued {
                message_id,
                delivery_state,
            } => json!({
                "messageId": message_id,
                "deliveryState": delivery_state,
            }),
            Self::WaitReady {
                receipt_id,
                source_receipt_id,
                targets,
            } => json!({
                "receiptId": receipt_id,
                "sourceReceiptId": source_receipt_id,
                "targets": targets,
            }),
            Self::WaitStopped { reason } => json!({
                "ready": false,
                "reason": reason,
            }),
            Self::Agents { agents } => json!({ "agents": agents }),
            Self::Interrupted {
                target_agent_id,
                status,
            } => json!({
                "targetAgentId": target_agent_id,
                "status": status,
            }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCollaborationResultPersistence {
    RuntimeCommits,
    /// The Host atomically advanced collaboration cursors and committed this exact wait
    /// ToolResult to the durable trace/model context. Runtime must update only its in-memory
    /// recorder and must not publish the same prefix a second time.
    PrecommittedWaitToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCollaborationExecutionOutput {
    pub result: AgentCollaborationToolResult,
    pub persistence: AgentCollaborationResultPersistence,
}

pub type AgentCollaborationExecutionFuture = Pin<
    Box<
        dyn Future<Output = crate::AgentResult<AgentCollaborationExecutionOutput>> + Send + 'static,
    >,
>;

/// Narrow Host boundary used by all six collaboration tools.
pub trait AgentCollaborationExecutor: Send + Sync {
    fn execute(
        &self,
        invocation: AgentCollaborationInvocation,
        control: AgentCollaborationExecutionControl,
    ) -> AgentCollaborationExecutionFuture;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_directory_is_sorted_bounded_and_has_no_secret_fields() {
        let templates = (0..40)
            .rev()
            .map(|index| AgentCollaborationTemplateSelector {
                agent_type: format!("type-{index:02}"),
                name: format!("Name {index}"),
                description: "x".repeat(700),
                model_display_name: "Display".to_string(),
                default_model_capabilities: ModelCapabilities {
                    image_input: index % 2 == 0,
                },
            })
            .collect();
        let models = (0..40)
            .rev()
            .map(|index| AgentCollaborationModelSelector {
                model_config_id: format!("model-{index:02}"),
                display_name: format!("Model {index}"),
                capabilities: ModelCapabilities {
                    image_input: index % 2 == 0,
                },
            })
            .collect();
        let directory = AgentCollaborationSelectorDirectory::bounded(templates, models);
        assert!(directory.truncated);
        assert!(directory.templates.len() <= AGENT_COLLABORATION_MAX_SELECTOR_ITEMS);
        assert!(directory.models.len() <= AGENT_COLLABORATION_MAX_SELECTOR_ITEMS);
        assert!(directory
            .templates
            .windows(2)
            .all(|pair| pair[0].agent_type < pair[1].agent_type));
        assert!(directory
            .models
            .windows(2)
            .all(|pair| pair[0].model_config_id < pair[1].model_config_id));
        let encoded = serde_json::to_string(&directory).unwrap();
        assert!(encoded.len() <= AGENT_COLLABORATION_MAX_SELECTOR_DIRECTORY_BYTES);
        for secret in [
            "apiKey",
            "apiToken",
            "apiUrl",
            "providerProfile",
            "instructions",
        ] {
            assert!(!encoded.contains(secret));
        }
        assert!(encoded.contains("defaultModelCapabilities"));
        assert!(encoded.contains("capabilities"));
    }

    #[test]
    fn selector_authorization_freezes_the_capability_of_the_effective_selector() {
        let directory = AgentCollaborationSelectorDirectory::bounded(
            vec![AgentCollaborationTemplateSelector {
                agent_type: "vision_reviewer".to_string(),
                name: "Vision reviewer".to_string(),
                description: "Review images".to_string(),
                model_display_name: "Template vision model".to_string(),
                default_model_capabilities: ModelCapabilities { image_input: true },
            }],
            vec![AgentCollaborationModelSelector {
                model_config_id: "text-model".to_string(),
                display_name: "Text model".to_string(),
                capabilities: ModelCapabilities { image_input: false },
            }],
        );
        let authorization = AgentCollaborationSelectorAuthorization::from_directory(&directory);
        assert_eq!(
            authorization.expected_model_capabilities(Some("vision_reviewer"), None),
            Some(ModelCapabilities { image_input: true })
        );
        assert_eq!(
            authorization.expected_model_capabilities(Some("vision_reviewer"), Some("text-model")),
            Some(ModelCapabilities { image_input: false })
        );
        assert_eq!(authorization.expected_model_capabilities(None, None), None);
    }

    #[test]
    fn wait_model_payload_matches_the_durable_repository_contract() {
        let value = AgentCollaborationToolResult::WaitReady {
            receipt_id: "receipt".into(),
            source_receipt_id: Some("source".into()),
            targets: Vec::new(),
        }
        .into_model_value()
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "receiptId": "receipt",
                "sourceReceiptId": "source",
                "targets": []
            })
        );
    }

    #[test]
    fn spawn_result_reports_the_frozen_child_model_capabilities() {
        let value = AgentCollaborationToolResult::Spawned {
            child_agent_id: "agent-vision".into(),
            task_path: "/root/vision".into(),
            model_display_name: "Vision".into(),
            model_capabilities: ModelCapabilities { image_input: true },
            status: AgentDisplayStatus::Queued,
        }
        .into_model_value()
        .unwrap();
        assert_eq!(value["modelCapabilities"]["imageInput"], true);
        assert_eq!(value["modelDisplayName"], "Vision");
    }
}
