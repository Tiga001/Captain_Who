//! Managed, application-packaged runtimes for reproducible artifact scripts.
//!
//! This module is deliberately a discovery and preflight boundary. It never
//! authorizes a command and never starts a process. Command authorization stays
//! with `run_command`; a successful resolution only replaces an ambiguous PATH
//! lookup with a revision-bound executable, fixed startup arguments, and a
//! minimal host-owned environment.

mod discovery;
mod types;

pub use discovery::{
    artifact_runtime_component_relative_path, ArtifactRuntimeDiscoveryOptions,
    ArtifactRuntimeProvider, ARTIFACT_RUNTIME_BUNDLE_VERSION, ARTIFACT_RUNTIME_NODE_VERSION,
    ARTIFACT_RUNTIME_PYTHON_VERSION, ARTIFACT_RUNTIME_RIPGREP_VERSION,
};
pub use types::{
    ArtifactRuntimeAvailability, ArtifactRuntimeBundleStatus, ArtifactRuntimeDependency,
    ArtifactRuntimeError, ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeKind,
    ArtifactRuntimePreflight, ArtifactRuntimeRecovery, ArtifactRuntimeRequirement,
    ArtifactRuntimeSource, ArtifactRuntimeStatus, ARTIFACT_RUNTIME_BUNDLE_STATUS_SCHEMA_VERSION,
    ARTIFACT_RUNTIME_PROVIDER_ID,
};
