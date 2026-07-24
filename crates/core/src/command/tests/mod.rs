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
        inputs: Vec::new(),
        runtime: None,
        runtime_binding: None,
    }
}

fn frozen_runtime_binding() -> crate::AgentCommandRuntimeBinding {
    let resolved_packages = vec![crate::AgentCommandRuntimeResolvedPackage {
        name: "python-docx".to_string(),
        version: "1.2.0".to_string(),
    }];
    crate::AgentCommandRuntimeBinding {
        schema_version: crate::AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile: crate::AgentCommandRuntimeProfile::Documents,
        profile_revision: runtime_profile_revision(
            crate::AgentCommandRuntimeProfile::Documents,
            crate::AgentCommandRuntimeKind::Python,
            &resolved_packages,
        ),
        provider_id: crate::artifact_runtime::ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: "test-bundle".to_string(),
        bundle_revision: "test-revision".to_string(),
        kind: crate::AgentCommandRuntimeKind::Python,
        runtime_version: "3.12.0".to_string(),
        runtime_fingerprint: "artifact-runtime-sha256-v1:test".to_string(),
        resolved_packages,
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
