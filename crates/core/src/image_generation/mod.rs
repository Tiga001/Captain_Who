//! Provider-neutral image-generation core.
//!
//! Configuration, credentials, normalized requests, provider execution, and eventual Artifact
//! publication are deliberately separate boundaries. Provider adapters never own artifact
//! downloads, and artifact transfers never receive provider credentials.

mod artifact;
pub mod configuration;
pub mod credential_store;
mod execution;
mod http;
mod provider;
mod seedream;
mod types;

pub(crate) use artifact::validate_staged_image;
pub use artifact::{
    ImageArtifactError, ImageArtifactErrorCode, ImageArtifactFormat, ImageArtifactHostRule,
    ImageArtifactNetworkPolicy, ImageArtifactPublicationStatus, ImageArtifactStoreConfig,
    ImageArtifactTransferPolicy, ImageGenerationArtifactCandidate, ImageGenerationArtifactStore,
    ManagedImageArtifactContent, ManagedImageGenerationArtifactStore, PreparedImageArtifact,
    PublishedImageArtifact, DEFAULT_IMAGE_ARTIFACT_CONNECT_TIMEOUT,
    DEFAULT_IMAGE_ARTIFACT_DNS_TIMEOUT, DEFAULT_IMAGE_ARTIFACT_DOWNLOAD_TIMEOUT,
    DEFAULT_IMAGE_ARTIFACT_MAX_BYTES, DEFAULT_IMAGE_ARTIFACT_MAX_REDIRECTS,
    MAX_IMAGE_ARTIFACT_DIMENSION, MAX_IMAGE_ARTIFACT_PIXELS,
};
pub use configuration::{
    CredentialReconciliationReport, ImageGenerationConfiguration,
    ImageGenerationConfigurationError, ImageGenerationConfigurationMutationOutcome,
    ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationService,
    ImageGenerationConfigurationUpdate, ImageGenerationCredentialMutation,
    ImageGenerationCredentialStatus, ImageGenerationReadiness,
};
#[cfg(target_os = "macos")]
pub use credential_store::NonInteractiveMacCredentialStore;
pub use credential_store::{
    CredentialDeleteOutcome, CredentialReference, CredentialSecret, CredentialStore,
    CredentialStoreBackend, CredentialStoreError, CredentialStoreOperation,
    DevelopmentFileCredentialStore, InMemoryCredentialStore, SystemCredentialStore,
    IMAGE_GENERATION_CREDENTIAL_SERVICE,
};
pub use execution::{
    ImageGenerationExecutionFailure, ImageGenerationExecutionFailureCode,
    ImageGenerationExecutionId, ImageGenerationExecutionLimits, ImageGenerationExecutionPhase,
    ImageGenerationExecutionReceipt, ImageGenerationExecutionRecoveryReport,
    ImageGenerationExecutionRequest, ImageGenerationExecutionResult,
    ImageGenerationExecutionService, ImageGenerationExecutionServiceError,
    ImageGenerationExecutionServiceErrorCode, ImageGenerationExecutionShutdownReport,
    ImageGenerationExecutionStatus, DEFAULT_IMAGE_GENERATION_CONFIGURATION_TIMEOUT,
    DEFAULT_IMAGE_GENERATION_EXECUTION_TIMEOUT, DEFAULT_IMAGE_GENERATION_MAX_ADMITTED,
    DEFAULT_IMAGE_GENERATION_MAX_CONCURRENT, IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
};
pub use http::{
    validate_endpoint, ImageHttpClientConfig, ImageHttpEndpointPolicy,
    DEFAULT_IMAGE_HTTP_CONNECT_TIMEOUT, DEFAULT_IMAGE_HTTP_MAX_RESPONSE_BYTES,
    DEFAULT_IMAGE_HTTP_REQUEST_TIMEOUT,
};
pub use provider::{
    ImageGenerationAdapterRegistry, ImageGenerationProvider, ImageGenerationProviderFactory,
    ImageGenerationProviderRegistry,
};
pub use seedream::{SmartMlSeedreamProvider, SmartMlSeedreamProviderFactory};
pub use types::{
    ImageGenerationAdapterId, ImageGenerationCapabilities, ImageGenerationDataUrlInput,
    ImageGenerationDefaults, ImageGenerationEditRequest, ImageGenerationError,
    ImageGenerationErrorCode, ImageGenerationGenerateRequest, ImageGenerationInputMediaType,
    ImageGenerationOperation, ImageGenerationOutputTransport, ImageGenerationProviderProfile,
    ImageGenerationRequest, ImageGenerationResult, ImageGenerationResultStatus,
    ImageGenerationSizePreset, ImageGenerationUrlOutput, NormalizedImageGenerationRequest,
    PreparedImageGenerationRequest, IMAGE_GENERATION_OUTPUT_COUNT,
    MAX_IMAGE_GENERATION_INPUT_BYTES, MAX_IMAGE_GENERATION_PROMPT_BYTES,
};
