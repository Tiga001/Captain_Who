mod bundled;
mod digest;
mod discovery;
mod github_acquisition;
mod github_source_resolution;
mod installation_service;
mod installation_workflow;
mod installed;
mod managed_fs;
mod managed_installer;
mod managed_store;
mod model;
mod origin;
mod package;
mod parser;
mod prepared;
mod resolver;
mod service;
mod source;
mod source_resolution;
mod workspace;

pub use bundled::{APPLICATION_BUNDLED_SKILL_SOURCE_ID, REPOSITORY_EVIDENCE_AUDITOR_LOCAL_ID};
pub use github_acquisition::{
    AcquiredGitHubSkill, GitHubAcquisitionError, GitHubAcquisitionErrorCode,
    GitHubAcquisitionSummary, GitHubAcquisitionTransport, GitHubArchiveRequest, GitHubCommit,
    GitHubNamedReference, GitHubReference, GitHubRepository, GitHubResolveRequest,
    GitHubSkillAcquirer, GitHubSkillLocation, GitHubSubdirectory, GitHubTransportError,
    GitHubWorkflowAcquisitionAdapter, ReqwestGitHubTransport, GITHUB_SKILL_ORIGIN_PROVIDER,
};
pub use github_source_resolution::GitHubInstallationSourceResolver;
pub use installation_service::{
    LocalSkillInstallRequest, LocalSkillUpdateRequest, SkillInstallationMutation,
    SkillInstallationOperation, SkillInstallationOutcome, SkillInstallationService,
    SkillInstallationServiceError, SkillUninstallRequest,
};
pub use installation_workflow::{
    SkillAcquisitionAdapter, SkillAcquisitionAdapterError, SkillAcquisitionAdapterErrorCode,
    SkillAcquisitionPresentation, SkillAcquisitionProvider, SkillAcquisitionProviderError,
    SkillAcquisitionSource, SkillAcquisitionSourceError, SkillInstallationCommitRequest,
    SkillInstallationCommitResult, SkillInstallationPackagePreview,
    SkillInstallationPreparationIntent, SkillInstallationPreparationRequest,
    SkillInstallationPreview, SkillInstallationWarning, SkillInstallationWarningCode,
    SkillInstallationWorkflow, SkillInstallationWorkflowConfig,
    SkillInstallationWorkflowConfigurationError, SkillInstallationWorkflowError,
    SkillPackageResourceSummary, SkillPreparationCancellation, SkillPreparationId,
    SkillPreparationIdError, SkillPreviewRevision, SkillPreviewRevisionError,
};
pub use installed::USER_INSTALLED_SKILL_SOURCE_ID;
pub use managed_installer::{
    ManagedSkillInstallOutcome, ManagedSkillInstallRequest, ManagedSkillInstaller,
    ManagedSkillInstallerError, ManagedSkillInstallerErrorCode, ManagedSkillMutation,
    ManagedSkillStoreCapacity, ManagedSkillUninstallOutcome, ManagedSkillUninstallRequest,
    ManagedSkillUpdateOutcome, ManagedSkillUpdateRequest,
};
pub use model::{
    ActivatedSkillSet, ResolvedSkill, ResolvedSkillPackage, SkillActivationError,
    SkillActivationPolicy, SkillActivationRevision, SkillActivationScope, SkillCatalog,
    SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillErrorCode, SkillId, SkillInstallationId, SkillMetadata,
    SkillProvenance, SkillRecovery, SkillReferenceError, SkillRegistrationError, SkillResolveError,
    SkillResolveRequest, SkillResourceDescriptor, SkillResourceIndex, SkillResourceKind,
    SkillRevision, SkillSelection, SkillSourceId, SkillSourceKind, SkillTrust,
    DEFAULT_MAX_ACTIVATED_SKILLS, DEFAULT_MAX_ACTIVATED_SKILL_BYTES, SKILL_PACKAGE_FORMAT_VERSION,
    SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
pub use origin::{SkillPackageOrigin, SkillPackageOriginError};
pub use prepared::{
    PreparedSkillPackage, SkillPackagePreparationError, LOCAL_DIRECTORY_SKILL_ORIGIN_PROVIDER,
};
pub use service::SkillsService;
pub use source_resolution::{
    ResolvedSkillPackagePreview, ResolvedSkillSource, SkillInstallationSourceLocator,
    SkillInstallationSourceResolver, SkillSourceResolution, SkillSourceResolutionCandidate,
    SkillSourceResolutionError, SkillSourceResolutionErrorCode, SkillSourceResolutionOutcome,
    SkillSourceResolutionPhase, SkillSourceResolutionRecovery, SkillSourceResolutionService,
    SkillSourceResolverConfigurationError, SkillSourceResolverId,
};
