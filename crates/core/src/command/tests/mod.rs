use super::*;
use crate::{AgentApprovalStatus, AgentCommandPermission, AgentReadPermission};
use tempfile::TempDir;

mod execution;
mod policy_basics;
mod policy_complex;

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
        runtime: None,
        runtime_binding: None,
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
