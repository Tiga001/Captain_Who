use crate::artifact_runtime::{
    ArtifactRuntimeError, ArtifactRuntimeKind, ArtifactRuntimePreflight, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery, ArtifactRuntimeRequirement, ARTIFACT_RUNTIME_PROVIDER_ID,
};
use crate::{
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimeProfile,
    AgentCommandRuntimeResolvedPackage, AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};

pub const COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH: &str = "artifactRuntime.bindingMismatch";
pub const COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE: &str =
    "artifactRuntime.runtimeBindingRequired";
const ERROR_MISSING_DEPENDENCIES: &str = "artifactRuntime.missingDependencies";

const PROFILE_REVISION_PREFIX: &str = "artifact-runtime-profile-sha256-v1:";

/// A stable, structured failure raised while a trusted host resolves a runtime profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRuntimeProfileError {
    code: String,
    recovery: String,
    message: String,
}

impl CommandRuntimeProfileError {
    fn new(
        code: impl Into<String>,
        recovery: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            recovery: recovery.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn recovery(&self) -> &str {
        &self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for CommandRuntimeProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for CommandRuntimeProfileError {}

/// Read-only approval-time resolver injected by the application host.
///
/// Future trusted profile registries can implement this boundary without changing the model-facing
/// `run_command` schema. It never authorizes a command and never exposes launch authority.
pub trait CommandRuntimeProfileResolver: Send + Sync {
    fn resolve_profile(
        &self,
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
    ) -> Result<AgentCommandRuntimeBinding, CommandRuntimeProfileError>;
}

impl CommandRuntimeProfileResolver for ArtifactRuntimeProvider {
    fn resolve_profile(
        &self,
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
    ) -> Result<AgentCommandRuntimeBinding, CommandRuntimeProfileError> {
        prepare_command_runtime_profile(self, profile, kind).map(|prepared| prepared.binding)
    }
}

pub(crate) struct PreparedCommandRuntimeProfile {
    pub binding: AgentCommandRuntimeBinding,
    pub invocation: crate::artifact_runtime::ArtifactRuntimeInvocation,
}

/// Resolves the exact versions from the verified component receipt and returns both a persistable
/// binding and a private invocation. Callers must never persist the invocation.
pub(crate) fn prepare_command_runtime_profile(
    provider: &ArtifactRuntimeProvider,
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
) -> Result<PreparedCommandRuntimeProfile, CommandRuntimeProfileError> {
    if profile == AgentCommandRuntimeProfile::Pdf && kind != AgentCommandRuntimeKind::Python {
        return Err(CommandRuntimeProfileError::new(
            "artifactRuntime.invalidRequest",
            ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
            "Managed PDF Runtime 只支持受管 Python 执行环境。",
        ));
    }
    let artifact_kind = artifact_kind(kind);
    let status = provider.runtime_status(artifact_kind);
    let mut resolved_packages = profile_package_names(profile, kind)
        .iter()
        .map(|name| {
            status
                .dependencies
                .iter()
                .find(|dependency| {
                    normalize_package_name(kind, &dependency.name)
                        == normalize_package_name(kind, name)
                })
                .map(|dependency| AgentCommandRuntimeResolvedPackage {
                    name: dependency.name.clone(),
                    version: dependency.version.clone(),
                })
                .ok_or_else(|| {
                    CommandRuntimeProfileError::new(
                        ERROR_MISSING_DEPENDENCIES,
                        ArtifactRuntimeRecovery::RepairComponent.stable_name(),
                        format!(
                            "Managed Artifact Runtime 的 {} profile 缺少后端要求的依赖 `{name}`。",
                            profile_name(profile)
                        ),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    canonicalize_packages(kind, &mut resolved_packages);

    let requirements = resolved_packages
        .iter()
        .map(|package| ArtifactRuntimeRequirement::exact(&package.name, &package.version))
        .collect::<Result<Vec<_>, _>>()
        .map_err(profile_provider_error)?;
    let invocation = match provider
        .preflight(artifact_kind, &requirements)
        .map_err(profile_provider_error)?
    {
        ArtifactRuntimePreflight::Ready(invocation) => invocation,
        ArtifactRuntimePreflight::MissingDependencies { missing, .. } => {
            let missing = missing
                .iter()
                .map(|requirement| match requirement.version() {
                    Some(version) => format!("{}@{version}", requirement.name()),
                    None => requirement.name().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CommandRuntimeProfileError::new(
                ERROR_MISSING_DEPENDENCIES,
                ArtifactRuntimeRecovery::RepairComponent.stable_name(),
                format!("Managed Artifact Runtime 缺少 profile 依赖：{missing}。"),
            ));
        }
    };

    let binding = AgentCommandRuntimeBinding {
        schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        profile,
        profile_revision: runtime_profile_revision(profile, kind, &resolved_packages),
        provider_id: invocation.provider_id().to_string(),
        bundle_version: invocation.bundle_version().to_string(),
        bundle_revision: invocation.bundle_revision().to_string(),
        kind,
        runtime_version: invocation.version().to_string(),
        runtime_fingerprint: invocation.runtime_fingerprint().to_string(),
        resolved_packages,
    };
    validate_command_runtime_binding(&binding)?;
    Ok(PreparedCommandRuntimeProfile {
        binding,
        invocation,
    })
}

pub(crate) fn validate_command_runtime_binding(
    binding: &AgentCommandRuntimeBinding,
) -> Result<(), CommandRuntimeProfileError> {
    if binding.schema_version != AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION {
        return Err(CommandRuntimeProfileError::new(
            COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
            "reprepare",
            "Managed Artifact Runtime binding 版本不受支持；请重新准备命令。",
        ));
    }
    if binding.provider_id != ARTIFACT_RUNTIME_PROVIDER_ID
        || binding.bundle_version.trim().is_empty()
        || binding.bundle_revision.trim().is_empty()
        || binding.runtime_version.trim().is_empty()
        || binding.runtime_fingerprint.trim().is_empty()
    {
        return Err(CommandRuntimeProfileError::new(
            COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
            "reprepare",
            "Managed Artifact Runtime binding 缺少有效的运行时身份。",
        ));
    }

    let expected_names = profile_package_names(binding.profile, binding.kind);
    if binding.resolved_packages.len() != expected_names.len()
        || binding
            .resolved_packages
            .iter()
            .any(|package| package.name.trim().is_empty() || package.version.trim().is_empty())
    {
        return Err(CommandRuntimeProfileError::new(
            COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
            "reprepare",
            "Managed Artifact Runtime binding 的 profile 依赖集合无效。",
        ));
    }
    let mut packages = binding.resolved_packages.clone();
    canonicalize_packages(binding.kind, &mut packages);
    if packages != binding.resolved_packages
        || packages
            .iter()
            .zip(expected_names)
            .any(|(actual, expected)| {
                normalize_package_name(binding.kind, &actual.name)
                    != normalize_package_name(binding.kind, expected)
            })
        || binding.profile_revision
            != runtime_profile_revision(binding.profile, binding.kind, &binding.resolved_packages)
    {
        return Err(CommandRuntimeProfileError::new(
            COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
            "reprepare",
            "Managed Artifact Runtime binding 与后端 profile 定义不一致。",
        ));
    }
    Ok(())
}

fn profile_package_names(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
) -> &'static [&'static str] {
    match (profile, kind) {
        (AgentCommandRuntimeProfile::Documents, AgentCommandRuntimeKind::Node) => &["docx"],
        (AgentCommandRuntimeProfile::Documents, AgentCommandRuntimeKind::Python) => {
            &["python-docx"]
        }
        (AgentCommandRuntimeProfile::Spreadsheets, AgentCommandRuntimeKind::Node) => &["exceljs"],
        (AgentCommandRuntimeProfile::Spreadsheets, AgentCommandRuntimeKind::Python) => {
            &["openpyxl", "xlsxwriter"]
        }
        (AgentCommandRuntimeProfile::Presentations, AgentCommandRuntimeKind::Node) => {
            &["pptxgenjs"]
        }
        (AgentCommandRuntimeProfile::Presentations, AgentCommandRuntimeKind::Python) => {
            &["python-pptx"]
        }
        (AgentCommandRuntimeProfile::Pdf, AgentCommandRuntimeKind::Python) => {
            &["pdfplumber", "pypdf", "pypdfium2", "reportlab"]
        }
        (AgentCommandRuntimeProfile::Pdf, AgentCommandRuntimeKind::Node) => &[],
    }
}

pub(crate) fn runtime_profile_revision(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
    packages: &[AgentCommandRuntimeResolvedPackage],
) -> String {
    let material = serde_json::to_vec(&serde_json::json!({
        "schemaVersion": AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
        "profile": profile,
        "kind": kind,
        "packages": packages,
    }))
    .expect("runtime profile revision material is serializable");
    let digest = Sha256::digest(material);
    format!("{PROFILE_REVISION_PREFIX}{digest:x}")
}

fn canonicalize_packages(
    kind: AgentCommandRuntimeKind,
    packages: &mut [AgentCommandRuntimeResolvedPackage],
) {
    packages.sort_by(|left, right| {
        normalize_package_name(kind, &left.name).cmp(&normalize_package_name(kind, &right.name))
    });
}

fn normalize_package_name(kind: AgentCommandRuntimeKind, name: &str) -> String {
    match kind {
        AgentCommandRuntimeKind::Node => name.to_ascii_lowercase(),
        AgentCommandRuntimeKind::Python => name.to_ascii_lowercase().replace('_', "-"),
    }
}

fn artifact_kind(kind: AgentCommandRuntimeKind) -> ArtifactRuntimeKind {
    match kind {
        AgentCommandRuntimeKind::Node => ArtifactRuntimeKind::Node,
        AgentCommandRuntimeKind::Python => ArtifactRuntimeKind::Python,
    }
}

fn profile_name(profile: AgentCommandRuntimeProfile) -> &'static str {
    match profile {
        AgentCommandRuntimeProfile::Documents => "documents",
        AgentCommandRuntimeProfile::Spreadsheets => "spreadsheets",
        AgentCommandRuntimeProfile::Presentations => "presentations",
        AgentCommandRuntimeProfile::Pdf => "pdf",
    }
}

fn profile_provider_error(error: ArtifactRuntimeError) -> CommandRuntimeProfileError {
    CommandRuntimeProfileError::new(
        format!("artifactRuntime.{}", error.code().stable_name()),
        error.recovery().stable_name(),
        match error.code() {
            crate::artifact_runtime::ArtifactRuntimeErrorCode::InvalidRequest => {
                "Managed Artifact Runtime profile 请求无效。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::Unavailable => {
                "Managed Artifact Runtime 组件不可用。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::InvalidComponent => {
                "Managed Artifact Runtime 组件格式无效；请修复或重新安装组件。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::IntegrityMismatch => {
                "Managed Artifact Runtime 未通过完整性校验；请修复或重新安装组件。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::UnsupportedTarget => {
                "Managed Artifact Runtime 不支持当前系统或架构。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::RuntimeConflict => {
                "Managed Artifact Runtime 组件与当前 profile 冲突。"
            }
            crate::artifact_runtime::ArtifactRuntimeErrorCode::Io => {
                "Managed Artifact Runtime 发生 I/O 错误。"
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(
        profile: AgentCommandRuntimeProfile,
        kind: AgentCommandRuntimeKind,
        packages: Vec<AgentCommandRuntimeResolvedPackage>,
    ) -> AgentCommandRuntimeBinding {
        AgentCommandRuntimeBinding {
            schema_version: AGENT_COMMAND_RUNTIME_BINDING_SCHEMA_VERSION,
            profile,
            profile_revision: runtime_profile_revision(profile, kind, &packages),
            provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            bundle_version: "test-bundle".to_string(),
            bundle_revision: "test-revision".to_string(),
            kind,
            runtime_version: "test-runtime".to_string(),
            runtime_fingerprint: "test-fingerprint".to_string(),
            resolved_packages: packages,
        }
    }

    #[test]
    fn all_profiles_have_deterministic_node_and_python_package_sets() {
        let cases = [
            (
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Node,
                vec!["docx"],
            ),
            (
                AgentCommandRuntimeProfile::Documents,
                AgentCommandRuntimeKind::Python,
                vec!["python-docx"],
            ),
            (
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Node,
                vec!["exceljs"],
            ),
            (
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Python,
                vec!["openpyxl", "xlsxwriter"],
            ),
            (
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
                vec!["pptxgenjs"],
            ),
            (
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Python,
                vec!["python-pptx"],
            ),
        ];
        for (profile, kind, expected) in cases {
            assert_eq!(profile_package_names(profile, kind), expected);
        }
    }

    #[test]
    fn binding_validation_rejects_package_or_revision_tampering() {
        let packages = vec![AgentCommandRuntimeResolvedPackage {
            name: "pptxgenjs".to_string(),
            version: "4.0.1".to_string(),
        }];
        let valid = binding(
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
            packages,
        );
        validate_command_runtime_binding(&valid).unwrap();

        let mut tampered = valid.clone();
        tampered.resolved_packages[0].version = "3.12.0".to_string();
        assert!(validate_command_runtime_binding(&tampered).is_err());

        let mut tampered = valid;
        tampered.profile_revision.push('0');
        assert!(validate_command_runtime_binding(&tampered).is_err());
    }
}
