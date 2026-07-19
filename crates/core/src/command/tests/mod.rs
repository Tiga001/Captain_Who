use super::*;
use crate::{AgentApprovalStatus, AgentCommandPermission, AgentReadPermission};
use std::sync::atomic::{AtomicU64, Ordering};

mod execution;
mod policy_basics;
mod policy_complex;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn request(command: &str, timeout_ms: Option<u64>) -> AgentCommandRequest {
    AgentCommandRequest {
        id: "tool-1".to_string(),
        command: command.to_string(),
        cwd: None,
        timeout_ms,
        approval_status: AgentApprovalStatus::Required,
        risk_level: Some(AgentCommandRiskLevel::ReadOnly),
        reason: None,
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
    path: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("mycopilot-command-test-{unique}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let path = path.canonicalize().unwrap();

        Self { path }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
