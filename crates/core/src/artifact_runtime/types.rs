use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

pub const ARTIFACT_RUNTIME_PROVIDER_ID: &str = "mycopilot.artifact-runtime";
pub const ARTIFACT_RUNTIME_BUNDLE_STATUS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactRuntimeKind {
    Node,
    Python,
}

impl ArtifactRuntimeKind {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Python => "python",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactRuntimeAvailability {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactRuntimeSource {
    ConfiguredComponent,
    PackagedComponent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRuntimeDependency {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRuntimeStatus {
    pub kind: ArtifactRuntimeKind,
    pub availability: ArtifactRuntimeAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<ArtifactRuntimeDependency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRuntimeBundleStatus {
    pub schema_version: u32,
    pub provider_id: String,
    pub availability: ArtifactRuntimeAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ArtifactRuntimeSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_revision: Option<String>,
    pub runtimes: Vec<ArtifactRuntimeStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRuntimeRequirement {
    name: String,
    version: Option<String>,
}

impl ArtifactRuntimeRequirement {
    pub fn new(
        name: impl Into<String>,
        version: Option<impl Into<String>>,
    ) -> Result<Self, ArtifactRuntimeError> {
        let name = name.into();
        validate_requirement_value(&name, "dependency name")?;
        let version = version.map(Into::into);
        if let Some(version) = version.as_deref() {
            validate_requirement_value(version, "dependency version")?;
        }
        Ok(Self { name, version })
    }

    pub fn any(name: impl Into<String>) -> Result<Self, ArtifactRuntimeError> {
        Self::new(name, None::<String>)
    }

    pub fn exact(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, ArtifactRuntimeError> {
        Self::new(name, Some(version))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

fn validate_requirement_value(value: &str, label: &str) -> Result<(), ArtifactRuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(ArtifactRuntimeError::new(
            ArtifactRuntimeErrorCode::InvalidRequest,
            ArtifactRuntimeRecovery::ChangeRequest,
            format!(
                "Artifact runtime {label} must be a trimmed, printable value of at most 128 bytes."
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRuntimeInvocation {
    provider_id: String,
    bundle_version: String,
    bundle_revision: String,
    runtime_fingerprint: String,
    kind: ArtifactRuntimeKind,
    version: String,
    executable: PathBuf,
    arguments_prefix: Vec<OsString>,
    environment: BTreeMap<OsString, OsString>,
}

impl ArtifactRuntimeInvocation {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        bundle_version: String,
        bundle_revision: String,
        runtime_fingerprint: String,
        kind: ArtifactRuntimeKind,
        version: String,
        executable: PathBuf,
        arguments_prefix: Vec<OsString>,
        environment: BTreeMap<OsString, OsString>,
    ) -> Self {
        Self {
            provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            bundle_version,
            bundle_revision,
            runtime_fingerprint,
            kind,
            version,
            executable,
            arguments_prefix,
            environment,
        }
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn bundle_version(&self) -> &str {
        &self.bundle_version
    }

    pub fn bundle_revision(&self) -> &str {
        &self.bundle_revision
    }

    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }

    pub fn kind(&self) -> ArtifactRuntimeKind {
        self.kind
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn executable(&self) -> &std::path::Path {
        &self.executable
    }

    pub fn arguments_prefix(&self) -> &[OsString] {
        &self.arguments_prefix
    }

    pub fn environment(&self) -> &BTreeMap<OsString, OsString> {
        &self.environment
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactRuntimePreflight {
    Ready(ArtifactRuntimeInvocation),
    MissingDependencies {
        status: ArtifactRuntimeStatus,
        missing: Vec<ArtifactRuntimeRequirement>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactRuntimeErrorCode {
    InvalidRequest,
    Unavailable,
    InvalidComponent,
    IntegrityMismatch,
    UnsupportedTarget,
    RuntimeConflict,
    Io,
}

impl ArtifactRuntimeErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalidRequest",
            Self::Unavailable => "unavailable",
            Self::InvalidComponent => "invalidComponent",
            Self::IntegrityMismatch => "integrityMismatch",
            Self::UnsupportedTarget => "unsupportedTarget",
            Self::RuntimeConflict => "runtimeConflict",
            Self::Io => "io",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactRuntimeRecovery {
    ChangeRequest,
    InstallComponent,
    RepairComponent,
    Retry,
}

impl ArtifactRuntimeRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ChangeRequest => "changeRequest",
            Self::InstallComponent => "installComponent",
            Self::RepairComponent => "repairComponent",
            Self::Retry => "retry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRuntimeError {
    code: ArtifactRuntimeErrorCode,
    recovery: ArtifactRuntimeRecovery,
    message: String,
}

impl ArtifactRuntimeError {
    pub fn new(
        code: ArtifactRuntimeErrorCode,
        recovery: ArtifactRuntimeRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            recovery,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ArtifactRuntimeErrorCode {
        self.code
    }

    pub fn recovery(&self) -> ArtifactRuntimeRecovery {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ArtifactRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for ArtifactRuntimeError {}
