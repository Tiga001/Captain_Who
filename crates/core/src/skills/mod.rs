mod acquisition_provenance;
mod agent_discovery;
mod bundled;
mod digest;
mod discovery;
mod github_acquisition;
mod github_source_resolution;
mod installation_service;
mod installation_session;
mod installation_workflow;
mod installed;
mod managed_fs;
mod managed_installer;
mod managed_store;
mod materialization;
mod model;
mod origin;
mod package;
mod parser;
mod prepared;
mod prepared_acquisition;
mod resolver;
mod resource_runtime;
mod script_runtime;
mod service;
mod source;
mod source_resolution;
mod workspace;

pub(crate) use digest::activation_revision_for_identities;

pub use acquisition_provenance::{
    SkillInstallationAuthority, SkillInstallationAuthorityView, SkillInstallationProvenance,
    SkillInstallationProvenanceError, SkillInstallationProvenanceView, SkillInstallationRefresh,
    SkillInstallationRefreshView, MAX_SKILL_PROVENANCE_PAYLOAD_BYTES,
};
pub use agent_discovery::{
    derive_skill_activation_ref, AgentDiscoverableSkill, AgentSkillDiscoverySnapshot,
    SkillDiscoverySnapshotError, AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
    DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
};
pub use bundled::{
    APPLICATION_BUNDLED_SKILL_SOURCE_ID, DOCUMENTS_LOCAL_ID, IMAGE_GENERATION_LOCAL_ID,
    PRESENTATIONS_LOCAL_ID, SKILL_INSTALLER_LOCAL_ID, SPREADSHEETS_LOCAL_ID,
};
pub use github_acquisition::{
    AcquiredGitHubSkill, GitHubAcquisitionError, GitHubAcquisitionErrorCode,
    GitHubAcquisitionSummary, GitHubAcquisitionTransport, GitHubArchiveRequest, GitHubCommit,
    GitHubNamedReference, GitHubReference, GitHubRepository, GitHubResolveRequest,
    GitHubSkillAcquirer, GitHubSkillLocation, GitHubSubdirectory, GitHubTransportError,
    GitHubWorkflowAcquisitionAdapter, ReqwestGitHubTransport, GITHUB_SKILL_ORIGIN_PROVIDER,
};
pub use github_source_resolution::GitHubInstallationSourceResolver;
pub use installation_service::{
    InstalledSkillInventory, InstalledSkillRecord, InstalledSkillRecordIssue,
    LocalSkillInstallRequest, LocalSkillUpdateExactRequest, LocalSkillUpdateRequest,
    SkillInstallationInventoryError, SkillInstallationMutation, SkillInstallationOperation,
    SkillInstallationOutcome, SkillInstallationService, SkillInstallationServiceError,
    SkillUninstallExactRequest, SkillUninstallRequest,
};
pub use installation_session::{
    SkillInstallationSessionConfig, SkillInstallationSessionConfigurationError,
    SkillInstallationSessionStore, DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
    DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES, DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES,
    DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES, DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
};
pub use installation_workflow::{
    InstalledGitHubTrackingReference, InstalledSkillSourcePresentation, SkillAcquisitionAdapter,
    SkillAcquisitionAdapterError, SkillAcquisitionAdapterErrorCode, SkillAcquisitionPresentation,
    SkillAcquisitionProvider, SkillAcquisitionProviderError, SkillAcquisitionSource,
    SkillAcquisitionSourceError, SkillInstallationCommitRequest, SkillInstallationCommitResult,
    SkillInstallationFrozenPreparation, SkillInstallationPackagePreview,
    SkillInstallationPreparationIntent, SkillInstallationPreparationRequest,
    SkillInstallationPreview, SkillInstallationWarning, SkillInstallationWarningCode,
    SkillInstallationWorkflow, SkillInstallationWorkflowConfig,
    SkillInstallationWorkflowConfigurationError, SkillInstallationWorkflowError,
    SkillPackageResourceSummary, SkillPreparationCancellation, SkillPreparationId,
    SkillPreparationIdError, SkillPreviewRevision, SkillPreviewRevisionError,
};
pub use installed::USER_INSTALLED_SKILL_SOURCE_ID;
pub use managed_installer::{
    ManagedSkillCommittedState, ManagedSkillInstallOutcome, ManagedSkillInstallRequest,
    ManagedSkillInstaller, ManagedSkillInstallerError, ManagedSkillInstallerErrorCode,
    ManagedSkillLegacyUninstallRequest, ManagedSkillMutation, ManagedSkillMutationResult,
    ManagedSkillStoreCapacity, ManagedSkillUninstallOutcome, ManagedSkillUninstallRequest,
    ManagedSkillUpdateOutcome, ManagedSkillUpdateRequest,
};
pub use materialization::{
    SkillMaterializationDestination, SkillMaterializationError, SkillMaterializationErrorCode,
    SkillMaterializationOutcome, SkillMaterializationRecovery, SkillMaterializationRequest,
    SkillMaterializationStatus, SkillMaterializedTreeEntry, SkillResourceMaterializer,
    SkillTemplateTreeMaterializationOutcome, SkillTemplateTreeMaterializationRequest,
    MAX_SKILL_MATERIALIZATION_FILE_BYTES, MAX_SKILL_MATERIALIZATION_TREE_BYTES,
    MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES, MAX_SKILL_MATERIALIZATION_TREE_FILES,
    SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX,
};
pub use model::{
    ActivatedSkillSet, ResolvedSkill, ResolvedSkillPackage, SkillActivationError,
    SkillActivationPolicy, SkillActivationRevision, SkillActivationScope, SkillCatalog,
    SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillErrorCode, SkillId, SkillInstallationId, SkillInstallationRevision,
    SkillMetadata, SkillProvenance, SkillRecovery, SkillReferenceError, SkillRegistrationError,
    SkillResolveError, SkillResolveRequest, SkillResourceDescriptor, SkillResourceIndex,
    SkillResourceKind, SkillRevision, SkillSelection, SkillSourceId, SkillSourceKind, SkillTrust,
    DEFAULT_MAX_ACTIVATED_SKILLS, DEFAULT_MAX_ACTIVATED_SKILL_BYTES,
    SKILL_INSTALLATION_REVISION_PREFIX, SKILL_PACKAGE_FORMAT_VERSION,
    SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
pub use origin::{SkillPackageOrigin, SkillPackageOriginError};
pub use prepared::{
    PreparedSkillPackage, SkillPackagePreparationError, LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER,
};
pub use prepared_acquisition::PreparedSkillAcquisition;
#[cfg(test)]
pub(crate) use resource_runtime::memory_resource_session_for_test;
pub use resource_runtime::{
    SkillPackageUri, SkillResourceError, SkillResourceErrorCode, SkillResourceListEntry,
    SkillResourceListOptions, SkillResourceListPage, SkillResourcePath, SkillResourceRecovery,
    SkillResourceSession, SkillResourceTextPage, SkillResourceTextReadOptions, SkillResourceUri,
    SkillResourceUriError, DEFAULT_SKILL_RESOURCE_LIST_PAGE_SIZE,
    DEFAULT_SKILL_RESOURCE_TEXT_PAGE_BYTES, MAX_SKILL_RESOURCE_LIST_PAGE_SIZE,
    MAX_SKILL_RESOURCE_TEXT_PAGE_BYTES, MAX_SKILL_RESOURCE_URI_BYTES,
};
pub use script_runtime::{
    execute_skill_python_script, preflight_skill_python_script, SkillScriptPreflightOutcome,
    SkillScriptReadyPlan, SkillScriptRuntimeError, SkillScriptRuntimeErrorCode,
    SkillScriptRuntimeRecovery, DEFAULT_SKILL_SCRIPT_TIMEOUT_MS, MAX_SKILL_SCRIPT_ARGUMENTS,
    MAX_SKILL_SCRIPT_ARGUMENT_BYTES, MAX_SKILL_SCRIPT_REQUIREMENTS, MAX_SKILL_SCRIPT_TIMEOUT_MS,
};
pub use service::SkillsService;
pub use source_resolution::{
    PreparedSkillSourceResolution, PreparedSkillSourceResolutionCandidate,
    ResolvedSkillPackagePreview, ResolvedSkillSource, SkillInstallationSourceLocator,
    SkillInstallationSourceResolver, SkillRegisteredSourceResolution, SkillSourceCandidateId,
    SkillSourceCandidateIdError, SkillSourceResolution, SkillSourceResolutionCancellation,
    SkillSourceResolutionCandidate, SkillSourceResolutionError, SkillSourceResolutionErrorCode,
    SkillSourceResolutionId, SkillSourceResolutionIdError, SkillSourceResolutionOutcome,
    SkillSourceResolutionPhase, SkillSourceResolutionRecovery, SkillSourceResolutionService,
    SkillSourceResolverConfigurationError, SkillSourceResolverId,
};
