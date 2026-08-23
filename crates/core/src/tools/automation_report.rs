use super::{AgentTool, ToolExecutionContext};
use crate::protocol::{
    AgentError, AgentResult, AgentToolApprovalMode, AgentToolDefinition, AgentToolSafety,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

const MAX_AUTOMATION_REPORT_SUMMARY_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReportKind {
    NoChange,
    ImportantUpdate,
    Completed,
}

impl AutomationReportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoChange => "no_change",
            Self::ImportantUpdate => "important_update",
            Self::Completed => "completed",
        }
    }
}

/// Host-owned persistence boundary for the automation-only structured report tool.
///
/// The runtime knows neither task configuration nor notification policy. It only validates the
/// model's closed report shape and commits it through this run-scoped capability.
pub trait AutomationReportSink: Send + Sync {
    fn record(&self, kind: AutomationReportKind, summary: &str) -> Result<(), String>;
}

pub(super) struct AutomationReportTool {
    sink: Arc<dyn AutomationReportSink>,
}

impl AutomationReportTool {
    pub(super) fn new(sink: Arc<dyn AutomationReportSink>) -> Self {
        Self { sink }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AutomationReportInput {
    kind: AutomationReportKind,
    summary: String,
}

impl AgentTool for AutomationReportTool {
    fn exposure(&self) -> super::AgentToolExposure {
        super::AgentToolExposure::Stable
    }

    fn permission_policy(&self) -> super::AgentToolPermissionPolicy {
        super::AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: "automation_report".to_string(),
            description: "Record the structured outcome of this scheduled automation. Call this once near the end of the run. Use no_change when monitoring found nothing notable, important_update when the user should be alerted to a meaningful change, or completed for an ordinary completed task. This tool only records this run's report and cannot modify automation configuration."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["no_change", "important_update", "completed"]
                    },
                    "summary": {
                        "type": "string",
                        "description": "A concise user-facing summary of the outcome."
                    }
                },
                "required": ["kind", "summary"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::ReadOnly,
            requires_workspace: false,
            requires_approval: false,
            approval_mode: AgentToolApprovalMode::Never,
        }
    }

    fn execute(&self, context: &ToolExecutionContext, args: Value) -> AgentResult<Value> {
        context.check_cancelled()?;
        let input: AutomationReportInput = serde_json::from_value(args).map_err(|error| {
            AgentError::new(format!("automation_report parameters are invalid: {error}"))
        })?;
        let summary = input.summary.trim();
        if summary.is_empty()
            || summary.len() > MAX_AUTOMATION_REPORT_SUMMARY_BYTES
            || summary
                .chars()
                .any(|character| character.is_control() && character != '\n')
        {
            return Err(AgentError::new(
                "automation_report summary must be non-empty, safe text of at most 2048 UTF-8 bytes.",
            ));
        }
        self.sink
            .record(input.kind, summary)
            .map_err(|_| AgentError::new("automation_report could not be persisted safely."))?;
        Ok(json!({
            "recorded": true,
            "kind": input.kind,
            "summary": summary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<(AutomationReportKind, String)>>);

    impl AutomationReportSink for RecordingSink {
        fn record(&self, kind: AutomationReportKind, summary: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((kind, summary.to_string()));
            Ok(())
        }
    }

    #[test]
    fn report_shape_is_closed_and_persisted_without_configuration_authority() {
        let sink = Arc::new(RecordingSink::default());
        let tool = AutomationReportTool::new(sink.clone());
        let context = ToolExecutionContext::from_run_context(None);
        let output = tool
            .execute(
                &context,
                json!({"kind": "important_update", "summary": "A release is blocked."}),
            )
            .unwrap();
        assert_eq!(output["recorded"], true);
        assert_eq!(
            sink.0.lock().unwrap().as_slice(),
            &[(
                AutomationReportKind::ImportantUpdate,
                "A release is blocked.".to_string()
            )]
        );
        assert!(tool
            .execute(
                &context,
                json!({"kind": "completed", "summary": "Done", "schedule": "daily"}),
            )
            .is_err());
    }
}
