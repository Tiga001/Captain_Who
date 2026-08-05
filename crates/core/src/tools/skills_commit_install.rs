use super::{
    AgentTool, AgentToolExposure, AgentToolPermissionPolicy, ToolCapabilityId,
    ToolExecutionContext, SKILL_INSTALLATION_CAPABILITY,
};
use crate::protocol::{
    AgentError, AgentProposedAction, AgentResult, AgentSkillInstallationRequest,
    AgentToolApprovalMode, AgentToolCall, AgentToolDefinition, AgentToolSafety,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

const TOOL_NAME: &str = "skills_commit_install";
const INSTALL_REF_PREFIX: &str = "skill_install_";
const INSTALL_REF_HEX_CHARS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSkillInstallationCommitPreparationRequest {
    pub conversation_id: String,
    pub run_id: String,
    pub action_id: String,
    pub install_ref: String,
}

/// Host-owned boundary which resolves an opaque reference into a frozen, presentation-safe
/// approval action. The returned action contains no authority that the model can manufacture.
pub trait AgentSkillInstallationCommitPreparer: Send + Sync {
    fn prepare_commit_action(
        &self,
        request: AgentSkillInstallationCommitPreparationRequest,
    ) -> AgentResult<AgentSkillInstallationRequest>;

    fn invalidate_commit_action(&self, action: &AgentSkillInstallationRequest) -> AgentResult<()>;
}

pub(super) struct SkillsCommitInstallTool {
    preparer: Arc<dyn AgentSkillInstallationCommitPreparer>,
}

impl SkillsCommitInstallTool {
    pub(super) fn new(preparer: Arc<dyn AgentSkillInstallationCommitPreparer>) -> Self {
        Self { preparer }
    }
}

impl AgentTool for SkillsCommitInstallTool {
    fn exposure(&self) -> AgentToolExposure {
        AgentToolExposure::RequiresCapability(ToolCapabilityId::application_owned(
            SKILL_INSTALLATION_CAPABILITY,
        ))
    }

    fn permission_policy(&self) -> AgentToolPermissionPolicy {
        AgentToolPermissionPolicy::Default
    }

    fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: TOOL_NAME.to_string(),
            description: "Request user approval to install the exact frozen Skill returned by skills_prepare_install. Call only after you have explained the inspected Skill, its source, resources, scripts, and warnings to the user. The only input is the opaque installRef; installation never occurs before approval."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "installRef": {
                        "type": "string",
                        "pattern": "^skill_install_[0-9a-f]{32}$",
                        "description": "Exact opaque installRef returned by skills_prepare_install."
                    }
                },
                "required": ["installRef"],
                "additionalProperties": false
            }),
            safety: AgentToolSafety::RequiresApproval,
            requires_workspace: false,
            requires_approval: true,
            approval_mode: AgentToolApprovalMode::Always,
        }
    }

    fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
        Err(AgentError::new(
            "skills_commit_install requires explicit user approval and Host execution.",
        ))
    }

    fn proposed_action(
        &self,
        context: &ToolExecutionContext,
        call: &AgentToolCall,
    ) -> AgentResult<AgentProposedAction> {
        let args: CommitArgs = serde_json::from_value(call.args.clone()).map_err(|error| {
            invalid_request(format!(
                "skills_commit_install arguments are invalid: {error}"
            ))
        })?;
        let install_ref = args.install_ref.trim();
        if !valid_install_ref(install_ref) {
            return Err(invalid_request(
                "skills_commit_install.installRef is not a valid opaque installation reference.",
            ));
        }
        let installation = self.preparer.prepare_commit_action(
            AgentSkillInstallationCommitPreparationRequest {
                conversation_id: context.conversation_id()?.to_string(),
                run_id: context.run_id()?.to_string(),
                action_id: call.id.clone(),
                install_ref: install_ref.to_string(),
            },
        )?;
        Ok(AgentProposedAction::SkillInstallation {
            installation: Box::new(installation),
        })
    }

    fn invalidate_proposed_action(&self, action: &AgentProposedAction) -> AgentResult<()> {
        if let AgentProposedAction::SkillInstallation { installation } = action {
            self.preparer.invalidate_commit_action(installation)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommitArgs {
    install_ref: String,
}

fn valid_install_ref(value: &str) -> bool {
    value
        .strip_prefix(INSTALL_REF_PREFIX)
        .is_some_and(|digest| {
            digest.len() == INSTALL_REF_HEX_CHARS
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

fn invalid_request(message: impl Into<String>) -> AgentError {
    AgentError::structured(
        "skill.installation.invalidCommitRequest",
        message,
        json!({
            "type": "skillInstallation",
            "code": "invalidCommitRequest",
            "recovery": "prepareAgain"
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentApprovalStatus, AgentPermissions, AgentRunContext, AgentSkillInstallationPreview,
        AgentSkillInstallationResourceSummary,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingPreparer {
        requests: Mutex<Vec<AgentSkillInstallationCommitPreparationRequest>>,
    }

    impl AgentSkillInstallationCommitPreparer for RecordingPreparer {
        fn prepare_commit_action(
            &self,
            request: AgentSkillInstallationCommitPreparationRequest,
        ) -> AgentResult<AgentSkillInstallationRequest> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(AgentSkillInstallationRequest {
                schema_version: crate::protocol::AGENT_SKILL_INSTALLATION_SCHEMA_VERSION,
                id: request.action_id,
                install_ref: request.install_ref,
                preview: AgentSkillInstallationPreview {
                    name: "fixture".to_string(),
                    description: "Fixture Skill".to_string(),
                    source_summary: json!({ "kind": "github" }),
                    resolved_revision: "0123456789abcdef".to_string(),
                    file_count: 1,
                    total_bytes: 128,
                    resource_summary: AgentSkillInstallationResourceSummary {
                        total: 0,
                        references: 0,
                        assets: 0,
                        scripts: 0,
                        bytes: 0,
                    },
                    contains_scripts: false,
                    warnings: Vec::new(),
                    compatibility: "compatible".to_string(),
                    operation: "install".to_string(),
                    impact: "addManagedSkill".to_string(),
                },
                approval_status: AgentApprovalStatus::Required,
                expires_at: u64::MAX,
            })
        }

        fn invalidate_commit_action(
            &self,
            _action: &AgentSkillInstallationRequest,
        ) -> AgentResult<()> {
            Ok(())
        }
    }

    fn context() -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions::default(),
        }))
        .with_runtime_services("run-1".to_string(), None)
    }

    #[test]
    fn schema_accepts_only_one_opaque_install_ref_and_always_requires_approval() {
        let tool = SkillsCommitInstallTool::new(Arc::new(RecordingPreparer::default()));
        let definition = tool.definition();

        assert_eq!(definition.name, TOOL_NAME);
        assert_eq!(definition.safety, AgentToolSafety::RequiresApproval);
        assert_eq!(definition.approval_mode, AgentToolApprovalMode::Always);
        assert_eq!(definition.input_schema["required"], json!(["installRef"]));
        assert_eq!(definition.input_schema["additionalProperties"], false);
        assert_eq!(
            definition.input_schema["properties"]
                .as_object()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn proposed_action_binds_conversation_run_and_call_identity_behind_the_ref() {
        let preparer = Arc::new(RecordingPreparer::default());
        let tool = SkillsCommitInstallTool::new(preparer.clone());
        let install_ref = format!("skill_install_{}", "a".repeat(32));
        let action = tool
            .proposed_action(
                &context(),
                &AgentToolCall {
                    id: "action-1".to_string(),
                    tool: TOOL_NAME.to_string(),
                    args: json!({ "installRef": install_ref }),
                    approval_status: AgentApprovalStatus::Required,
                    reason: None,
                },
            )
            .unwrap();

        let AgentProposedAction::SkillInstallation { installation } = action else {
            panic!("expected typed Skill installation action");
        };
        assert_eq!(installation.id, "action-1");
        let requests = preparer.requests.lock().unwrap();
        assert_eq!(requests[0].conversation_id, "conversation-1");
        assert_eq!(requests[0].run_id, "run-1");
        assert_eq!(requests[0].action_id, "action-1");
    }

    #[test]
    fn malformed_refs_and_extra_model_fields_fail_before_the_host_boundary() {
        let preparer = Arc::new(RecordingPreparer::default());
        let tool = SkillsCommitInstallTool::new(preparer.clone());
        for args in [
            json!({ "installRef": "skill_install_not-opaque" }),
            json!({
                "installRef": format!("skill_install_{}", "a".repeat(32)),
                "warningAcknowledgement": true
            }),
        ] {
            assert!(tool
                .proposed_action(
                    &context(),
                    &AgentToolCall {
                        id: "action-invalid".to_string(),
                        tool: TOOL_NAME.to_string(),
                        args,
                        approval_status: AgentApprovalStatus::Required,
                        reason: None,
                    },
                )
                .is_err());
        }
        assert!(preparer.requests.lock().unwrap().is_empty());
    }
}
