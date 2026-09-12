use super::*;
use crate::{AgentApprovalStatus, AgentCommandPermission, AgentReadPermission};
use tempfile::TempDir;

mod multi_workspace;
mod policy_basics;
mod policy_complex;
mod session_manager;

fn request(command: &str, timeout_ms: Option<u64>) -> AgentCommandRequest {
    AgentCommandRequest {
        id: "tool-1".to_string(),
        command: command.to_string(),
        cwd: None,
        timeout_ms,
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(AgentCommandRiskLevel::ReadOnly),
        reason: None,
        observe: None,
        inputs: Vec::new(),
        runtime_binding: None,
        managed_office_script: None,
    }
}

fn policy_permissions(
    read: AgentReadPermission,
    command_safety: AgentCommandSafetyPolicy,
) -> AgentPermissions {
    AgentPermissions {
        read,
        command_safety,
        ..AgentPermissions::default()
    }
}

struct TestWorkspace {
    _directory: TempDir,
    path: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("mycopilot-command-test-")
            .tempdir()
            .unwrap();
        let path = directory.path().canonicalize().unwrap();

        Self {
            _directory: directory,
            path,
        }
    }
}
