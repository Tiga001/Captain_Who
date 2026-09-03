use super::*;

const AUTOMATION_RESULT_PREVIEW_MAX_BYTES: usize = 8_192;
const AUTOMATION_ERROR_MESSAGE_MAX_BYTES: usize = 4_096;

/// Host-owned metadata for one automation execution.
///
/// This value never crosses the Renderer Agent RPC and is never prepended to the user's prompt.
/// The durable `automation_runs` row remains authoritative across restart; this structure is the
/// exact correlation context handed from the scheduler to the shared HumanRoot executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutomationExecutionContext {
    pub(crate) automation_id: String,
    pub(crate) automation_run_id: String,
    pub(crate) scheduled_for: i64,
    pub(crate) last_run_at: Option<i64>,
    pub(crate) trigger_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutomationHumanRootDestination {
    NewChat {
        project_id: Option<String>,
        model_id: String,
    },
    ExistingChat {
        conversation_id: String,
    },
}

/// Trusted scheduler input for one HumanRoot Turn.
///
/// Unlike `AgentConversationTurnInput`, every identity and authority-bearing value here is loaded
/// from the frozen automation run snapshot by core-server. There is deliberately no transport
/// handler for this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutomationHumanRootTurnStart {
    pub(crate) context: AutomationExecutionContext,
    pub(crate) admission_token: String,
    pub(crate) config_revision: i64,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) destination: AutomationHumanRootDestination,
    pub(crate) permission_mode: String,
    pub(crate) permissions: mycopilot_core::AgentPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutomationTurnObservation {
    Running,
    WaitingForApproval,
    Terminal {
        status: ConversationTurnTraceTerminalStatus,
        result_preview: Option<String>,
        error_code: Option<String>,
        error_message: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutomationHumanRootStartError {
    RetryableCapacity,
    RetryableConversationBusy,
    TargetInvalid { code: &'static str, message: String },
    Fatal(String),
}

impl std::fmt::Display for AutomationHumanRootStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RetryableCapacity => {
                formatter.write_str("Agent execution capacity is currently busy")
            }
            Self::RetryableConversationBusy => {
                formatter.write_str("the target conversation has an active HumanRoot turn")
            }
            Self::TargetInvalid { message, .. } | Self::Fatal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for AutomationHumanRootStartError {}

struct ResolvedAutomationDestination {
    conversation_id: Option<String>,
    project_id: Option<String>,
    model_id: String,
    title: Option<String>,
}

/// Run-scoped, storage-backed capability exposed only after the Automation run has crossed the
/// exactly-once admission boundary. Keeping just the durable Automation run identity here makes
/// the same capability safe to recreate after an approval/restart continuation without adding
/// Automation metadata to the public Agent input or checkpoint protocol.
struct StorageAutomationReportSink {
    storage: Arc<StorageService>,
    automation_run_id: String,
}

impl AutomationReportSink for StorageAutomationReportSink {
    fn record(&self, kind: AutomationReportKind, summary: &str) -> Result<(), String> {
        match self.storage.record_automation_report(
            &self.automation_run_id,
            kind.as_str(),
            summary,
            now_ms(),
        )? {
            mycopilot_core::storage::automation_repository::AutomationRunMutationOutcome::Updated(
                _,
            ) => Ok(()),
            mycopilot_core::storage::automation_repository::AutomationRunMutationOutcome::Stale(
                _,
            ) => Err("the automation run no longer accepts structured reports".to_string()),
        }
    }
}

impl AgentService {
    /// Resolves the optional report capability from durable run ownership.
    ///
    /// Ordinary HumanRoot and child runs have no row keyed by their Agent run ID, so they never
    /// receive `automation_report`. Approval recovery calls this again with the original durable
    /// Agent run ID and therefore restores exactly the same run-scoped authority.
    pub(super) fn automation_report_sink_for_agent_run_id(
        &self,
        agent_run_id: &str,
    ) -> Result<Option<Arc<dyn AutomationReportSink>>, String> {
        self.storage
            .get_automation_run_by_agent_run_id(agent_run_id)
            .map(|run| {
                run.map(|run| {
                    Arc::new(StorageAutomationReportSink {
                        storage: Arc::clone(&self.storage),
                        automation_run_id: run.id,
                    }) as Arc<dyn AutomationReportSink>
                })
            })
    }

    /// Starts a scheduler-authenticated automation as a normal HumanRoot Turn.
    ///
    /// The method intentionally reuses the public HumanRoot preparation/runtime pipeline, including
    /// provider compatibility, Skills, MCP, permissions, approval persistence, Trace, messages,
    /// cancellation, and the process-wide Turn concurrency gate. The only additional authority is
    /// the automation admission token, consumed in the same SQLite transaction as the Conversation
    /// messages and empty in-progress Trace.
    pub(crate) fn start_automation_human_root_turn(
        &self,
        start: AutomationHumanRootTurnStart,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AutomationHumanRootStartError> {
        validate_automation_start(&start)
            .map_err(|error| AutomationHumanRootStartError::Fatal(error.to_string()))?;
        let destination_snapshot = start.destination.clone();
        let resolved =
            self.resolve_automation_human_root_destination(&start.destination, start.title.trim())?;

        if let Some(conversation_id) = resolved.conversation_id.as_deref() {
            if self
                .has_conversation_turn_occupancy(conversation_id)
                .map_err(AutomationHumanRootStartError::Fatal)?
            {
                return Err(AutomationHumanRootStartError::RetryableConversationBusy);
            }
        }
        let global_permit = self
            .turn_concurrency_gate()
            .try_acquire()
            .map_err(|_| AutomationHumanRootStartError::RetryableCapacity)?;

        let input = AgentConversationTurnInput {
            conversation_id: resolved.conversation_id,
            project_id: resolved.project_id,
            model_id: resolved.model_id,
            context_window_indicator_enabled: true,
            content: start.prompt,
            attachments: Vec::new(),
            skills: Vec::new(),
            title: resolved.title,
            user_message_id: None,
            assistant_message_id: None,
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions: start.permissions,
        };
        let admission = AutomationHumanRootAdmission {
            context: start.context,
            admission_token: start.admission_token,
            config_revision: start.config_revision,
            permission_mode: start.permission_mode,
            global_permit: Some(global_permit),
        };
        match self.start_human_root_turn_internal(input, None, Some(admission), notifications) {
            Ok(output) => Ok(output),
            Err(error)
                if error.data().and_then(|data| data["code"].as_str())
                    == Some("conversation_busy") =>
            {
                Err(AutomationHumanRootStartError::RetryableConversationBusy)
            }
            Err(error)
                if error.data().and_then(|data| data["code"].as_str())
                    == Some("permission_disabled") =>
            {
                Err(AutomationHumanRootStartError::TargetInvalid {
                    code: "permission_disabled",
                    message: mycopilot_core::storage::automation_repository::AUTOMATION_PERMISSION_DISABLED_MESSAGE.to_string(),
                })
            }
            Err(error) => {
                // A target can disappear between the read-only resolution above and atomic Turn
                // admission. Re-resolve it once so that race is a repairable blocked task rather
                // than a generic execution failure.
                match self.resolve_automation_human_root_destination(
                    &destination_snapshot,
                    start.title.trim(),
                ) {
                    Err(target @ AutomationHumanRootStartError::TargetInvalid { .. }) => {
                        Err(target)
                    }
                    _ => Err(AutomationHumanRootStartError::Fatal(error.to_string())),
                }
            }
        }
    }

    fn resolve_automation_human_root_destination(
        &self,
        destination: &AutomationHumanRootDestination,
        title: &str,
    ) -> Result<ResolvedAutomationDestination, AutomationHumanRootStartError> {
        let projects = self
            .storage
            .load_projects()
            .map_err(AutomationHumanRootStartError::Fatal)?;
        let models = self
            .storage
            .load_model_settings()
            .map_err(AutomationHumanRootStartError::Fatal)?
            .map(|settings| settings.models)
            .unwrap_or_default();
        match destination {
            AutomationHumanRootDestination::NewChat {
                project_id,
                model_id,
            } => {
                if let Some(project_id) = project_id {
                    let project = projects
                        .iter()
                        .find(|project| project.id == *project_id)
                        .ok_or_else(|| AutomationHumanRootStartError::TargetInvalid {
                            code: "project_missing",
                            message: "The selected project no longer exists.".to_string(),
                        })?;
                    if project
                        .path
                        .as_deref()
                        .is_none_or(|path| !std::path::Path::new(path).is_dir())
                    {
                        return Err(AutomationHumanRootStartError::TargetInvalid {
                            code: "project_path_missing",
                            message: "The selected project path is not available.".to_string(),
                        });
                    }
                }
                validate_automation_model(&models, model_id)?;
                Ok(ResolvedAutomationDestination {
                    conversation_id: None,
                    project_id: project_id.clone(),
                    model_id: model_id.clone(),
                    title: Some(title.to_string()),
                })
            }
            AutomationHumanRootDestination::ExistingChat { conversation_id } => {
                let conversation = self
                    .storage
                    .load_conversation(conversation_id)
                    .map_err(AutomationHumanRootStartError::Fatal)?
                    .ok_or_else(|| AutomationHumanRootStartError::TargetInvalid {
                        code: "target_missing",
                        message: "The target conversation is unavailable or archived.".to_string(),
                    })?;
                if conversation.archived_at.is_some() {
                    return Err(AutomationHumanRootStartError::TargetInvalid {
                        code: "target_archived",
                        message: "The target conversation is archived.".to_string(),
                    });
                }
                self.authorize_user_conversation_write(conversation_id)
                    .map_err(|_| AutomationHumanRootStartError::TargetInvalid {
                        code: "target_not_writable",
                        message: "The target conversation is not writable.".to_string(),
                    })?;
                if let Some(agent) = self
                    .storage
                    .get_agent_node_by_conversation(conversation_id)
                    .map_err(|error| AutomationHumanRootStartError::Fatal(error.to_string()))?
                {
                    if agent.parent_agent_id.is_some()
                        || agent.lifecycle != mycopilot_core::AgentLifecycle::Active
                    {
                        return Err(AutomationHumanRootStartError::TargetInvalid {
                            code: "target_not_root",
                            message: "The target conversation is not an active root chat."
                                .to_string(),
                        });
                    }
                }
                if let Some(project_id) = conversation.project_id.as_deref() {
                    let project = projects
                        .iter()
                        .find(|project| project.id == project_id)
                        .ok_or_else(|| AutomationHumanRootStartError::TargetInvalid {
                            code: "project_missing",
                            message: "The target conversation project no longer exists."
                                .to_string(),
                        })?;
                    if project
                        .path
                        .as_deref()
                        .is_none_or(|path| !std::path::Path::new(path).is_dir())
                    {
                        return Err(AutomationHumanRootStartError::TargetInvalid {
                            code: "project_path_missing",
                            message: "The target conversation project path is unavailable."
                                .to_string(),
                        });
                    }
                }
                let model_id = conversation.model_id.clone().ok_or_else(|| {
                    AutomationHumanRootStartError::TargetInvalid {
                        code: "model_missing",
                        message: "The target conversation has no model.".to_string(),
                    }
                })?;
                validate_automation_model(&models, &model_id)?;
                Ok(ResolvedAutomationDestination {
                    conversation_id: Some(conversation.id),
                    project_id: conversation.project_id,
                    model_id,
                    title: None,
                })
            }
        }
    }

    /// Returns the durable execution boundary used by the automation watcher.
    ///
    /// Process-local `agent.event` notifications are only an accelerator. The Trace decides
    /// terminal state and durable pending actions decide whether an in-progress Turn is waiting
    /// for approval, making this safe to call immediately after restart.
    pub(crate) fn automation_turn_state(
        &self,
        agent_run_id: &str,
        assistant_message_id: &str,
    ) -> Result<AutomationTurnObservation, String> {
        let trace = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| "automation agent Turn trace is missing".to_string())?;
        if trace.run_id != agent_run_id || trace.assistant_message_id != assistant_message_id {
            return Err(
                "automation agent Turn identity does not match its durable trace".to_string(),
            );
        }

        if trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress {
            let waiting = self
                .storage
                .list_pending_agent_actions()?
                .into_iter()
                .any(|action| {
                    action.run_id == agent_run_id
                        && matches!(action.status.as_str(), "pending" | "approved")
                });
            return Ok(if waiting {
                AutomationTurnObservation::WaitingForApproval
            } else {
                AutomationTurnObservation::Running
            });
        }

        let assistant_content = self
            .storage
            .load_conversation(&trace.conversation_id)?
            .and_then(|conversation| {
                conversation
                    .messages
                    .into_iter()
                    .find(|message| message.id == assistant_message_id)
                    .map(|message| message.content)
            });
        let result_preview = assistant_content
            .as_deref()
            .map(|content| bounded_safe_text(content, AUTOMATION_RESULT_PREVIEW_MAX_BYTES))
            .filter(|content| !content.is_empty());
        let (error_code, error_message) = match trace.terminal_status {
            ConversationTurnTraceTerminalStatus::Completed => (None, None),
            ConversationTurnTraceTerminalStatus::Failed => (
                Some("agent_turn_failed".to_string()),
                Some(bounded_safe_text(
                    trace
                        .terminal_error
                        .as_deref()
                        .unwrap_or("The automated Agent turn failed."),
                    AUTOMATION_ERROR_MESSAGE_MAX_BYTES,
                )),
            ),
            ConversationTurnTraceTerminalStatus::Cancelled => (
                Some("agent_turn_cancelled".to_string()),
                Some("The automated Agent turn was cancelled.".to_string()),
            ),
            ConversationTurnTraceTerminalStatus::InProgress => unreachable!(),
        };
        Ok(AutomationTurnObservation::Terminal {
            status: trace.terminal_status,
            result_preview,
            error_code,
            error_message,
        })
    }

    /// Best-effort Host cancellation for an automation-owned Agent run.
    ///
    /// Automation deletion is already authorized by the automation CAS and must not pretend to be
    /// a Renderer/user cancellation of an arbitrary collaboration run.
    pub(crate) fn cancel_automation_agent_run(&self, agent_run_id: &str) -> Result<bool, String> {
        self.interrupt_agent_wake_run(agent_run_id)
            .map(AgentRunCancellationOutcome::any_effect)
    }
}

#[derive(Debug, Clone)]
pub(super) struct AutomationHumanRootAdmission {
    pub(super) context: AutomationExecutionContext,
    pub(super) admission_token: String,
    pub(super) config_revision: i64,
    pub(super) permission_mode: String,
    pub(super) global_permit:
        Option<crate::application::agent_dispatcher::AgentTurnConcurrencyPermit>,
}

fn validate_automation_start(
    start: &AutomationHumanRootTurnStart,
) -> Result<(), AgentServiceError> {
    for (field, value) in [
        ("automationId", start.context.automation_id.as_str()),
        ("automationRunId", start.context.automation_run_id.as_str()),
        ("admissionToken", start.admission_token.as_str()),
        ("triggerKind", start.context.trigger_kind.as_str()),
    ] {
        if value.is_empty() || value.trim() != value || value.len() > 256 {
            return Err(format!("invalid automation HumanRoot {field}").into());
        }
    }
    if !matches!(
        start.context.trigger_kind.as_str(),
        "scheduled" | "manual" | "recovery"
    ) {
        return Err("invalid automation HumanRoot triggerKind"
            .to_string()
            .into());
    }
    if start.context.scheduled_for < 0
        || start.context.last_run_at.is_some_and(|value| value < 0)
        || start.config_revision <= 0
    {
        return Err("invalid automation HumanRoot execution metadata"
            .to_string()
            .into());
    }
    if !matches!(
        start.permission_mode.as_str(),
        "default" | "full" | "custom"
    ) {
        return Err("invalid automation HumanRoot permission mode"
            .to_string()
            .into());
    }
    if start.title.trim().is_empty() || start.prompt.trim().is_empty() {
        return Err("automation HumanRoot title and prompt cannot be empty"
            .to_string()
            .into());
    }
    match &start.destination {
        AutomationHumanRootDestination::NewChat {
            project_id,
            model_id,
        } => {
            if model_id.is_empty() || model_id.trim() != model_id {
                return Err("invalid automation HumanRoot model identity"
                    .to_string()
                    .into());
            }
            if project_id
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.trim() != value)
            {
                return Err("invalid automation HumanRoot project identity"
                    .to_string()
                    .into());
            }
        }
        AutomationHumanRootDestination::ExistingChat { conversation_id }
            if conversation_id.is_empty() || conversation_id.trim() != conversation_id =>
        {
            return Err("invalid automation HumanRoot conversation identity"
                .to_string()
                .into());
        }
        AutomationHumanRootDestination::ExistingChat { .. } => {}
    }
    Ok(())
}

fn validate_automation_model(
    models: &[mycopilot_core::storage::models::ModelConfigRecord],
    model_id: &str,
) -> Result<(), AutomationHumanRootStartError> {
    let model = models
        .iter()
        .find(|model| model.id == model_id)
        .ok_or_else(|| AutomationHumanRootStartError::TargetInvalid {
            code: "model_missing",
            message: "The selected model no longer exists.".to_string(),
        })?;
    if !model.enabled {
        return Err(AutomationHumanRootStartError::TargetInvalid {
            code: "model_disabled",
            message: "The selected model is disabled.".to_string(),
        });
    }
    Ok(())
}

fn bounded_safe_text(value: &str, max_bytes: usize) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.len() <= max_bytes {
        return sanitized.trim().to_string();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !sanitized.is_char_boundary(boundary) {
        boundary -= 1;
    }
    sanitized[..boundary].trim().to_string()
}
