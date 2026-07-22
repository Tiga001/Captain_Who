//! Provider-neutral image-generation core.
//!
//! Configuration, credentials, normalized requests, provider execution, and eventual Artifact
//! publication are deliberately separate boundaries. This module owns only the first three plus
//! the SmartML/Seedream HTTP adapter. It does not download provider output URLs or publish files.

pub mod configuration;
pub mod credential_store;
mod http;
mod provider;
mod seedream;
mod types;

pub use configuration::{
    CredentialReconciliationReport, ImageGenerationConfiguration,
    ImageGenerationConfigurationError, ImageGenerationConfigurationMutationOutcome,
    ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationService,
    ImageGenerationConfigurationUpdate, ImageGenerationCredentialMutation,
    ImageGenerationCredentialStatus, ImageGenerationReadiness,
};
pub use credential_store::{
    CredentialDeleteOutcome, CredentialReference, CredentialSecret, CredentialStore,
    CredentialStoreError, CredentialStoreOperation, InMemoryCredentialStore, SystemCredentialStore,
    IMAGE_GENERATION_CREDENTIAL_SERVICE,
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
