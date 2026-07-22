//! Backend-authoritative image-generation provider configuration.
//!
//! This service coordinates SQLite profile state with the native credential store. Secret writes
//! are staged in SQLite before reaching the platform store, and replaced references are queued
//! for cleanup in the same transaction that publishes the new profile. Startup reconciliation
//! therefore has enough durable evidence to finish either side of an interrupted mutation.

use super::credential_store::{
    CredentialReference, CredentialSecret, CredentialStore, CredentialStoreError,
};
use super::types::{
    ImageGenerationAdapterId, ImageGenerationCapabilities, ImageGenerationDefaults,
    ImageGenerationProviderProfile, ImageGenerationSizePreset,
};
use crate::storage::image_generation_repository::{
    ImageGenerationCredentialStageOutcome, ImageGenerationProfileCompareAndSetOutcome,
    DEFAULT_IMAGE_GENERATION_PROFILE_ID, IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
};
use crate::storage::models::ImageGenerationProfileRecord;
use crate::storage::service::StorageService;
use reqwest::Url;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

const CONFIGURATION_REVISION_PREFIX: &str = "image-generation:v1:";
const MAX_ENDPOINT_URL_BYTES: usize = 2_048;
const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_CREDENTIAL_BYTES: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationCredentialStatus {
    Missing,
    Configured,
    /// A durable credential reference exists, but the native store cannot currently verify it.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationReadiness {
    Disabled,
    MissingEndpoint,
    MissingModel,
    MissingCredential,
    CredentialUnavailable,
    ReadyUnverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationConfiguration {
    pub adapter_id: ImageGenerationAdapterId,
    pub endpoint_url: String,
    pub model_id: String,
    pub capabilities: ImageGenerationCapabilities,
    pub defaults: ImageGenerationDefaults,
    pub credential_status: ImageGenerationCredentialStatus,
    pub enabled: bool,
    pub readiness: ImageGenerationReadiness,
    pub revision: String,
    pub generation: u64,
}

impl ImageGenerationConfiguration {
    pub fn provider_profile(
        &self,
    ) -> Result<ImageGenerationProviderProfile, ImageGenerationConfigurationError> {
        if self.readiness != ImageGenerationReadiness::ReadyUnverified {
            return Err(ImageGenerationConfigurationError::ConfigurationIncomplete(
                self.readiness,
            ));
        }
        ImageGenerationProviderProfile::new(
            DEFAULT_IMAGE_GENERATION_PROFILE_ID,
            self.adapter_id.clone(),
            self.endpoint_url.clone(),
            self.model_id.clone(),
            self.generation,
            self.capabilities.clone(),
            self.defaults,
        )
        .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)
    }
}

pub enum ImageGenerationCredentialMutation {
    Keep,
    Replace(CredentialSecret),
    Clear,
}

impl fmt::Debug for ImageGenerationCredentialMutation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keep => formatter.write_str("Keep"),
            Self::Replace(_) => formatter.write_str("Replace([REDACTED])"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Debug)]
pub struct ImageGenerationConfigurationUpdate {
    pub expected_revision: String,
    pub adapter_id: ImageGenerationAdapterId,
    pub endpoint_url: String,
    pub model_id: String,
    pub text_to_image: bool,
    pub image_to_image: bool,
    pub defaults: ImageGenerationDefaults,
    pub credential_mutation: ImageGenerationCredentialMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationConfigurationMutationOutcome {
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenerationConfigurationMutationResult {
    pub outcome: ImageGenerationConfigurationMutationOutcome,
    pub configuration: ImageGenerationConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationConfigurationError {
    InvalidRevision,
    RevisionConflict { current_revision: String },
    UnsupportedAdapter,
    TextToImageRequired,
    MissingEndpoint,
    InvalidEndpoint,
    InsecureEndpoint,
    MissingModel,
    InvalidModelId,
    MissingCredential,
    InvalidCredential,
    CredentialReplacementRequired,
    ConfigurationIncomplete(ImageGenerationReadiness),
    InvalidConfiguration,
    StorageUnavailable,
    CredentialStoreUnavailable,
    CommitIndeterminate,
    Unavailable,
}

impl fmt::Display for ImageGenerationConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRevision => "image-generation configuration revision is invalid",
            Self::RevisionConflict { .. } => "image-generation configuration changed",
            Self::UnsupportedAdapter => "image-generation adapter is unsupported",
            Self::TextToImageRequired => "text-to-image is a required capability",
            Self::MissingEndpoint => "image-generation endpoint is missing",
            Self::InvalidEndpoint => "image-generation endpoint is invalid",
            Self::InsecureEndpoint => "image-generation endpoint must use HTTPS",
            Self::MissingModel => "image-generation model id is missing",
            Self::InvalidModelId => "image-generation model id is invalid",
            Self::MissingCredential => "image-generation credential is missing",
            Self::InvalidCredential => "image-generation credential is invalid",
            Self::CredentialReplacementRequired => {
                "changing the image-generation adapter requires replacing or clearing its credential"
            }
            Self::ConfigurationIncomplete(_) => "image-generation configuration is incomplete",
            Self::InvalidConfiguration => "image-generation configuration is invalid",
            Self::StorageUnavailable => "image-generation configuration storage is unavailable",
            Self::CredentialStoreUnavailable => "image-generation credential store is unavailable",
            Self::CommitIndeterminate => {
                "image-generation configuration commit outcome is indeterminate"
            }
            Self::Unavailable => "image-generation configuration service is unavailable",
        };
        formatter.write_str(message)
    }
}

impl Error for ImageGenerationConfigurationError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CredentialReconciliationReport {
    pub completed_staging: usize,
    pub removed_orphaned_credentials: usize,
    pub removed_retired_credentials: usize,
}

pub struct ImageGenerationConfigurationService {
    storage: Arc<StorageService>,
    credentials: Arc<dyn CredentialStore>,
}

/// A single-use, revision-consistent execution binding.
///
/// This value is deliberately crate-private, non-cloneable, and non-serializable. The execution
/// service keeps it only through the provider request and drops the zeroizing secret before it
/// starts downloading the provider's output URL.
pub(crate) struct ImageGenerationExecutionSnapshot {
    pub(crate) profile: ImageGenerationProviderProfile,
    pub(crate) credential: CredentialSecret,
}

impl fmt::Debug for ImageGenerationExecutionSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationExecutionSnapshot")
            .field("profile", &self.profile)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Debug for ImageGenerationConfigurationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationConfigurationService")
            .finish_non_exhaustive()
    }
}

impl ImageGenerationConfigurationService {
    #[must_use]
    pub fn new(storage: Arc<StorageService>, credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            storage,
            credentials,
        }
    }

    pub fn get_configuration(
        &self,
    ) -> Result<ImageGenerationConfiguration, ImageGenerationConfigurationError> {
        let record = self.load_record()?;
        self.configuration_from_record(record.as_ref())
    }

    /// Resolves a profile and credential that are proven to belong to the same configuration
    /// revision.
    ///
    /// Native credential reads are not transactional with SQLite. The service therefore reads
    /// the profile before and after Keychain access and retries once if either the generation or
    /// credential reference changed. It never exposes the persisted credential reference to the
    /// execution layer.
    pub(crate) fn resolve_execution_snapshot(
        &self,
    ) -> Result<ImageGenerationExecutionSnapshot, ImageGenerationConfigurationError> {
        for _ in 0..2 {
            let before = self.load_record()?;
            let Some(before_record) = before.as_ref() else {
                return Err(ImageGenerationConfigurationError::ConfigurationIncomplete(
                    ImageGenerationReadiness::Disabled,
                ));
            };
            let preliminary = self.configuration_from_record_with_credential_status(
                Some(before_record),
                ImageGenerationCredentialStatus::Configured,
            )?;
            if !preliminary.enabled {
                return Err(ImageGenerationConfigurationError::ConfigurationIncomplete(
                    ImageGenerationReadiness::Disabled,
                ));
            }
            validate_enabled_configuration(
                true,
                &preliminary.endpoint_url,
                &preliminary.model_id,
                if before_record.credential_ref.is_some() {
                    ImageGenerationCredentialStatus::Configured
                } else {
                    ImageGenerationCredentialStatus::Missing
                },
            )?;
            let reference = before_record
                .credential_ref
                .as_deref()
                .ok_or(ImageGenerationConfigurationError::MissingCredential)
                .and_then(|value| {
                    CredentialReference::parse(value)
                        .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)
                })?;
            let credential = self
                .credentials
                .get(&reference)
                .map_err(map_credential_error)?
                .ok_or(ImageGenerationConfigurationError::MissingCredential)?;
            let after = self.load_record()?;
            if after.as_ref() == before.as_ref() {
                let configuration = self.configuration_from_record_with_credential_status(
                    before.as_ref(),
                    ImageGenerationCredentialStatus::Configured,
                )?;
                return Ok(ImageGenerationExecutionSnapshot {
                    profile: configuration.provider_profile()?,
                    credential,
                });
            }
            // `credential` is zeroized here before attempting to resolve the new revision.
            drop(credential);
        }
        let current = self.load_record()?;
        Err(revision_conflict(current.as_ref()))
    }

    pub fn update_configuration(
        &self,
        mut update: ImageGenerationConfigurationUpdate,
    ) -> Result<ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationError> {
        let expected_generation = parse_revision(&update.expected_revision)?;
        let current_record = self.load_record()?;
        ensure_expected_generation(current_record.as_ref(), expected_generation)?;
        validate_update(&update)?;
        validate_credential_adapter_transition(current_record.as_ref(), &update)?;

        let final_credential_status = match &update.credential_mutation {
            // Only `keep` depends on the previous native-store entry. Replace and clear are the
            // recovery paths when that entry is unavailable or corrupt, so they must not read it.
            ImageGenerationCredentialMutation::Keep => {
                self.credential_status(current_record.as_ref())?
            }
            ImageGenerationCredentialMutation::Replace(secret) => {
                let valid = secret.with_secret_bytes(|bytes| {
                    !bytes.is_empty()
                        && bytes.len() <= MAX_CREDENTIAL_BYTES
                        && std::str::from_utf8(bytes).is_ok_and(|value| {
                            !value.chars().any(|character| {
                                character.is_whitespace() || character.is_control()
                            })
                        })
                });
                if !valid {
                    return Err(ImageGenerationConfigurationError::InvalidCredential);
                }
                ImageGenerationCredentialStatus::Configured
            }
            ImageGenerationCredentialMutation::Clear => ImageGenerationCredentialStatus::Missing,
        };
        let enabled = current_record.as_ref().is_some_and(|record| record.enabled);
        validate_enabled_configuration(
            enabled,
            &update.endpoint_url,
            &update.model_id,
            final_credential_status,
        )?;

        if is_same_configuration(current_record.as_ref(), &update, final_credential_status) {
            return Ok(ImageGenerationConfigurationMutationResult {
                outcome: ImageGenerationConfigurationMutationOutcome::AlreadyCurrent,
                configuration: self.configuration_from_record_with_credential_status(
                    current_record.as_ref(),
                    final_credential_status,
                )?,
            });
        }

        let credential_mutation = std::mem::replace(
            &mut update.credential_mutation,
            ImageGenerationCredentialMutation::Keep,
        );
        match credential_mutation {
            ImageGenerationCredentialMutation::Replace(secret) => self
                .replace_credential_and_profile(
                    expected_generation,
                    current_record.as_ref(),
                    update,
                    secret,
                ),
            ImageGenerationCredentialMutation::Keep => {
                let credential_ref = current_record
                    .as_ref()
                    .and_then(|record| record.credential_ref.clone());
                self.commit_profile(
                    expected_generation,
                    current_record.as_ref(),
                    &update,
                    credential_ref,
                    final_credential_status,
                )
            }
            ImageGenerationCredentialMutation::Clear => self.commit_profile(
                expected_generation,
                current_record.as_ref(),
                &update,
                None,
                final_credential_status,
            ),
        }
    }

    pub fn set_enabled(
        &self,
        expected_revision: &str,
        enabled: bool,
    ) -> Result<ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationError> {
        let expected_generation = parse_revision(expected_revision)?;
        let current_record = self.load_record()?;
        ensure_expected_generation(current_record.as_ref(), expected_generation)?;
        let current_enabled = current_record.as_ref().is_some_and(|record| record.enabled);
        if current_enabled == enabled {
            return Ok(ImageGenerationConfigurationMutationResult {
                outcome: ImageGenerationConfigurationMutationOutcome::AlreadyCurrent,
                configuration: self.configuration_from_record(current_record.as_ref())?,
            });
        }
        // Enabling must prove that the referenced secret actually exists. Disabling is a
        // fail-safe operation and must remain possible while the native credential backend is
        // unavailable, so it returns the recoverable snapshot status instead of failing the
        // mutation merely because the old secret cannot be read.
        let credential_status = if enabled {
            self.credential_status(current_record.as_ref())?
        } else {
            self.credential_status_for_snapshot(current_record.as_ref())?
        };
        let current_configuration = self.configuration_from_record_with_credential_status(
            current_record.as_ref(),
            credential_status,
        )?;
        if enabled {
            validate_enabled_configuration(
                true,
                &current_configuration.endpoint_url,
                &current_configuration.model_id,
                current_configuration.credential_status,
            )?;
        }

        let mut replacement = current_record.unwrap_or_else(default_record);
        replacement.enabled = enabled;
        self.commit_record(expected_generation, &replacement, credential_status)
    }

    /// Finishes credential mutations interrupted between SQLite and the native credential store.
    pub fn reconcile_credentials(
        &self,
    ) -> Result<CredentialReconciliationReport, ImageGenerationConfigurationError> {
        let mut report = CredentialReconciliationReport::default();
        let staging = self
            .storage
            .list_image_generation_credential_staging()
            .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?;
        for staged in staging {
            if staged.is_active {
                self.storage
                    .complete_image_generation_credential_staging(&staged.credential_ref)
                    .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?;
                report.completed_staging += 1;
                continue;
            }
            let reference = CredentialReference::parse(staged.credential_ref.clone())
                .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)?;
            self.credentials
                .delete(&reference)
                .map_err(map_credential_error)?;
            self.storage
                .complete_image_generation_credential_staging(&staged.credential_ref)
                .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?;
            report.removed_orphaned_credentials += 1;
        }
        self.drain_credential_cleanup(&mut report)?;
        Ok(report)
    }

    fn replace_credential_and_profile(
        &self,
        expected_generation: u64,
        current: Option<&ImageGenerationProfileRecord>,
        update: ImageGenerationConfigurationUpdate,
        secret: CredentialSecret,
    ) -> Result<ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationError> {
        let reference = CredentialReference::new_opaque();
        match self
            .storage
            .stage_image_generation_credential(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                expected_generation,
                reference.as_str(),
            )
            .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?
        {
            ImageGenerationCredentialStageOutcome::Staged => {}
            ImageGenerationCredentialStageOutcome::Conflict(actual) => {
                return Err(revision_conflict(actual.as_ref()));
            }
        }
        if let Err(error) = self.credentials.replace(&reference, secret) {
            if self.credentials.delete(&reference).is_ok() {
                let _ = self
                    .storage
                    .complete_image_generation_credential_staging(reference.as_str());
            }
            return Err(map_credential_error(error));
        }

        let committed = self.commit_profile(
            expected_generation,
            current,
            &update,
            Some(reference.as_str().to_string()),
            ImageGenerationCredentialStatus::Configured,
        );
        match committed {
            Ok(result) => {
                if self
                    .storage
                    .complete_image_generation_credential_staging(reference.as_str())
                    .is_err()
                {
                    return Err(ImageGenerationConfigurationError::CommitIndeterminate);
                }
                Ok(result)
            }
            Err(ImageGenerationConfigurationError::CommitIndeterminate) => {
                // Never delete an indeterminate credential. SQLite may already reference it.
                // The durable staging row lets startup reconciliation decide whether the secret
                // is active or orphaned after the database becomes readable again.
                Err(ImageGenerationConfigurationError::CommitIndeterminate)
            }
            Err(error) => {
                // Retain the staging row until the secret is gone. Startup reconciliation can
                // distinguish an active committed reference from an orphan after a crash.
                if self.credentials.delete(&reference).is_ok() {
                    let _ = self
                        .storage
                        .complete_image_generation_credential_staging(reference.as_str());
                }
                Err(error)
            }
        }
    }

    fn commit_profile(
        &self,
        expected_generation: u64,
        current: Option<&ImageGenerationProfileRecord>,
        update: &ImageGenerationConfigurationUpdate,
        credential_ref: Option<String>,
        credential_status: ImageGenerationCredentialStatus,
    ) -> Result<ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationError> {
        let mut replacement = current.cloned().unwrap_or_else(default_record);
        replacement.adapter_id = update.adapter_id.as_str().to_string();
        replacement.endpoint_url = update.endpoint_url.trim().to_string();
        replacement.model_id = update.model_id.trim().to_string();
        replacement.credential_ref = credential_ref;
        replacement.text_to_image = true;
        replacement.image_to_image = update.image_to_image;
        replacement.default_size_preset = update.defaults.size_preset.to_string();
        replacement.default_watermark = update.defaults.watermark;
        self.commit_record(expected_generation, &replacement, credential_status)
    }

    fn commit_record(
        &self,
        expected_generation: u64,
        replacement: &ImageGenerationProfileRecord,
        credential_status: ImageGenerationCredentialStatus,
    ) -> Result<ImageGenerationConfigurationMutationResult, ImageGenerationConfigurationError> {
        let outcome = self
            .storage
            .compare_and_set_image_generation_profile(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                expected_generation,
                replacement,
            )
            .map_err(|_| ImageGenerationConfigurationError::CommitIndeterminate)?;
        match outcome {
            ImageGenerationProfileCompareAndSetOutcome::Updated(record) => {
                // Credential access already occurred before the CAS. Do not introduce a second
                // native-store failure after SQLite has committed, because that would turn a
                // definite commit into an ambiguous client-visible failure.
                let configuration = self.configuration_from_record_with_credential_status(
                    Some(&record),
                    credential_status,
                )?;
                let mut report = CredentialReconciliationReport::default();
                // Cleanup failure does not roll back the published profile. The durable queue is
                // retried at startup and on the next successful mutation.
                let _ = self.drain_credential_cleanup(&mut report);
                Ok(ImageGenerationConfigurationMutationResult {
                    outcome: ImageGenerationConfigurationMutationOutcome::Updated,
                    configuration,
                })
            }
            ImageGenerationProfileCompareAndSetOutcome::Conflict(actual) => {
                Err(revision_conflict(actual.as_ref()))
            }
        }
    }

    fn load_record(
        &self,
    ) -> Result<Option<ImageGenerationProfileRecord>, ImageGenerationConfigurationError> {
        self.storage
            .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
            .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)
    }

    fn configuration_from_record(
        &self,
        record: Option<&ImageGenerationProfileRecord>,
    ) -> Result<ImageGenerationConfiguration, ImageGenerationConfigurationError> {
        let credential_status = self.credential_status_for_snapshot(record)?;
        self.configuration_from_record_with_credential_status(record, credential_status)
    }

    fn configuration_from_record_with_credential_status(
        &self,
        record: Option<&ImageGenerationProfileRecord>,
        credential_status: ImageGenerationCredentialStatus,
    ) -> Result<ImageGenerationConfiguration, ImageGenerationConfigurationError> {
        let record = record.cloned().unwrap_or_else(default_record);
        if record.schema_version != IMAGE_GENERATION_PROFILE_SCHEMA_VERSION {
            return Err(ImageGenerationConfigurationError::InvalidConfiguration);
        }
        let adapter_id = ImageGenerationAdapterId::try_from(record.adapter_id.as_str())
            .map_err(|_| ImageGenerationConfigurationError::UnsupportedAdapter)?;
        if adapter_id != ImageGenerationAdapterId::SmartMlSeedream {
            return Err(ImageGenerationConfigurationError::UnsupportedAdapter);
        }
        if !record.text_to_image {
            return Err(ImageGenerationConfigurationError::TextToImageRequired);
        }
        let defaults = ImageGenerationDefaults {
            size_preset: ImageGenerationSizePreset::try_from(record.default_size_preset.as_str())
                .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)?,
            watermark: record.default_watermark,
        };
        let capabilities = ImageGenerationCapabilities::smartml_seedream(record.image_to_image);
        let readiness = derive_readiness(
            record.enabled,
            &record.endpoint_url,
            &record.model_id,
            credential_status,
        );
        Ok(ImageGenerationConfiguration {
            adapter_id,
            endpoint_url: record.endpoint_url,
            model_id: record.model_id,
            capabilities,
            defaults,
            credential_status,
            enabled: record.enabled,
            readiness,
            revision: revision(record.generation),
            generation: record.generation,
        })
    }

    fn credential_status(
        &self,
        record: Option<&ImageGenerationProfileRecord>,
    ) -> Result<ImageGenerationCredentialStatus, ImageGenerationConfigurationError> {
        let Some(reference) = record.and_then(|record| record.credential_ref.as_deref()) else {
            return Ok(ImageGenerationCredentialStatus::Missing);
        };
        let reference = CredentialReference::parse(reference)
            .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)?;
        self.credentials
            .get(&reference)
            .map(|secret| {
                if secret.is_some() {
                    ImageGenerationCredentialStatus::Configured
                } else {
                    ImageGenerationCredentialStatus::Missing
                }
            })
            .map_err(map_credential_error)
    }

    /// Produces a secret-free snapshot even when the native credential backend is temporarily
    /// inaccessible. This keeps the authoritative revision available so callers can disable,
    /// clear, or replace a broken credential. Operations that rely on the existing secret still
    /// use `credential_status` and fail closed.
    fn credential_status_for_snapshot(
        &self,
        record: Option<&ImageGenerationProfileRecord>,
    ) -> Result<ImageGenerationCredentialStatus, ImageGenerationConfigurationError> {
        let Some(reference) = record.and_then(|record| record.credential_ref.as_deref()) else {
            return Ok(ImageGenerationCredentialStatus::Missing);
        };
        let reference = CredentialReference::parse(reference)
            .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)?;
        Ok(match self.credentials.get(&reference) {
            Ok(Some(_)) => ImageGenerationCredentialStatus::Configured,
            Ok(None) => ImageGenerationCredentialStatus::Missing,
            Err(_) => ImageGenerationCredentialStatus::Unavailable,
        })
    }

    fn drain_credential_cleanup(
        &self,
        report: &mut CredentialReconciliationReport,
    ) -> Result<(), ImageGenerationConfigurationError> {
        let cleanup = self
            .storage
            .list_image_generation_credential_cleanup()
            .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?;
        for value in cleanup {
            let reference = CredentialReference::parse(value.clone())
                .map_err(|_| ImageGenerationConfigurationError::InvalidConfiguration)?;
            self.credentials
                .delete(&reference)
                .map_err(map_credential_error)?;
            self.storage
                .complete_image_generation_credential_cleanup(&value)
                .map_err(|_| ImageGenerationConfigurationError::StorageUnavailable)?;
            report.removed_retired_credentials += 1;
        }
        Ok(())
    }
}

fn default_record() -> ImageGenerationProfileRecord {
    ImageGenerationProfileRecord {
        id: DEFAULT_IMAGE_GENERATION_PROFILE_ID.to_string(),
        schema_version: IMAGE_GENERATION_PROFILE_SCHEMA_VERSION,
        adapter_id: ImageGenerationAdapterId::SmartMlSeedream
            .as_str()
            .to_string(),
        endpoint_url: String::new(),
        model_id: String::new(),
        credential_ref: None,
        enabled: false,
        text_to_image: true,
        image_to_image: false,
        default_size_preset: ImageGenerationSizePreset::TwoK.to_string(),
        default_watermark: true,
        generation: 0,
        created_at: 0,
        updated_at: 0,
    }
}

fn validate_update(
    update: &ImageGenerationConfigurationUpdate,
) -> Result<(), ImageGenerationConfigurationError> {
    if update.adapter_id != ImageGenerationAdapterId::SmartMlSeedream {
        return Err(ImageGenerationConfigurationError::UnsupportedAdapter);
    }
    if !update.text_to_image {
        return Err(ImageGenerationConfigurationError::TextToImageRequired);
    }
    if update.defaults.size_preset != ImageGenerationSizePreset::TwoK {
        return Err(ImageGenerationConfigurationError::InvalidConfiguration);
    }
    validate_endpoint(&update.endpoint_url)?;
    validate_model_id(&update.model_id)
}

fn validate_credential_adapter_transition(
    current: Option<&ImageGenerationProfileRecord>,
    update: &ImageGenerationConfigurationUpdate,
) -> Result<(), ImageGenerationConfigurationError> {
    let Some(current) = current else {
        return Ok(());
    };
    if current.adapter_id != update.adapter_id.as_str()
        && current.credential_ref.is_some()
        && matches!(
            update.credential_mutation,
            ImageGenerationCredentialMutation::Keep
        )
    {
        return Err(ImageGenerationConfigurationError::CredentialReplacementRequired);
    }
    Ok(())
}

fn validate_endpoint(value: &str) -> Result<(), ImageGenerationConfigurationError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > MAX_ENDPOINT_URL_BYTES || value.chars().any(char::is_control) {
        return Err(ImageGenerationConfigurationError::InvalidEndpoint);
    }
    let url = Url::parse(value).map_err(|_| ImageGenerationConfigurationError::InvalidEndpoint)?;
    if url.scheme() != "https" {
        return Err(ImageGenerationConfigurationError::InsecureEndpoint);
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err(ImageGenerationConfigurationError::InvalidEndpoint);
    }
    if url.host_str().is_none() {
        return Err(ImageGenerationConfigurationError::InvalidEndpoint);
    }
    Ok(())
}

fn validate_model_id(value: &str) -> Result<(), ImageGenerationConfigurationError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(());
    }
    if value.len() > MAX_MODEL_ID_BYTES || value.chars().any(char::is_control) {
        return Err(ImageGenerationConfigurationError::InvalidModelId);
    }
    Ok(())
}

fn validate_enabled_configuration(
    enabled: bool,
    endpoint_url: &str,
    model_id: &str,
    credential_status: ImageGenerationCredentialStatus,
) -> Result<(), ImageGenerationConfigurationError> {
    if !enabled {
        return Ok(());
    }
    if endpoint_url.trim().is_empty() {
        return Err(ImageGenerationConfigurationError::MissingEndpoint);
    }
    if model_id.trim().is_empty() {
        return Err(ImageGenerationConfigurationError::MissingModel);
    }
    validate_endpoint(endpoint_url)?;
    validate_model_id(model_id)?;
    match credential_status {
        ImageGenerationCredentialStatus::Missing => {
            return Err(ImageGenerationConfigurationError::MissingCredential)
        }
        ImageGenerationCredentialStatus::Unavailable => {
            return Err(ImageGenerationConfigurationError::CredentialStoreUnavailable)
        }
        ImageGenerationCredentialStatus::Configured => {}
    }
    Ok(())
}

fn derive_readiness(
    enabled: bool,
    endpoint_url: &str,
    model_id: &str,
    credential_status: ImageGenerationCredentialStatus,
) -> ImageGenerationReadiness {
    if !enabled {
        ImageGenerationReadiness::Disabled
    } else if endpoint_url.trim().is_empty() {
        ImageGenerationReadiness::MissingEndpoint
    } else if model_id.trim().is_empty() {
        ImageGenerationReadiness::MissingModel
    } else {
        match credential_status {
            ImageGenerationCredentialStatus::Missing => ImageGenerationReadiness::MissingCredential,
            ImageGenerationCredentialStatus::Unavailable => {
                ImageGenerationReadiness::CredentialUnavailable
            }
            ImageGenerationCredentialStatus::Configured => {
                ImageGenerationReadiness::ReadyUnverified
            }
        }
    }
}

fn is_same_configuration(
    current: Option<&ImageGenerationProfileRecord>,
    update: &ImageGenerationConfigurationUpdate,
    final_credential_status: ImageGenerationCredentialStatus,
) -> bool {
    let current = current.cloned().unwrap_or_else(default_record);
    current.adapter_id == update.adapter_id.as_str()
        && current.endpoint_url == update.endpoint_url.trim()
        && current.model_id == update.model_id.trim()
        && current.text_to_image
        && current.image_to_image == update.image_to_image
        && current.default_size_preset == update.defaults.size_preset.to_string()
        && current.default_watermark == update.defaults.watermark
        && match &update.credential_mutation {
            ImageGenerationCredentialMutation::Keep => true,
            ImageGenerationCredentialMutation::Clear => {
                final_credential_status == ImageGenerationCredentialStatus::Missing
                    && current.credential_ref.is_none()
            }
            ImageGenerationCredentialMutation::Replace(_) => false,
        }
}

fn parse_revision(value: &str) -> Result<u64, ImageGenerationConfigurationError> {
    value
        .strip_prefix(CONFIGURATION_REVISION_PREFIX)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(ImageGenerationConfigurationError::InvalidRevision)
}

fn revision(generation: u64) -> String {
    format!("{CONFIGURATION_REVISION_PREFIX}{generation}")
}

fn ensure_expected_generation(
    current: Option<&ImageGenerationProfileRecord>,
    expected_generation: u64,
) -> Result<(), ImageGenerationConfigurationError> {
    let actual = current.map_or(0, |record| record.generation);
    if actual == expected_generation {
        Ok(())
    } else {
        Err(ImageGenerationConfigurationError::RevisionConflict {
            current_revision: revision(actual),
        })
    }
}

fn revision_conflict(
    current: Option<&ImageGenerationProfileRecord>,
) -> ImageGenerationConfigurationError {
    ImageGenerationConfigurationError::RevisionConflict {
        current_revision: revision(current.map_or(0, |record| record.generation)),
    }
}

fn map_credential_error(_: CredentialStoreError) -> ImageGenerationConfigurationError {
    ImageGenerationConfigurationError::CredentialStoreUnavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::credential_store::{
        CredentialDeleteOutcome, CredentialStoreOperation, InMemoryCredentialStore,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::tempdir;

    struct UnavailableCredentialStore;

    struct RotatingCredentialStore {
        delegate: InMemoryCredentialStore,
        storage: Arc<StorageService>,
        armed: AtomicBool,
    }

    #[derive(Default)]
    struct UnreadableCredentialStore {
        delegate: InMemoryCredentialStore,
    }

    impl CredentialStore for UnavailableCredentialStore {
        fn replace(
            &self,
            _reference: &CredentialReference,
            _secret: CredentialSecret,
        ) -> Result<(), CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Replace,
            })
        }

        fn get(
            &self,
            _reference: &CredentialReference,
        ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Get,
            })
        }

        fn delete(
            &self,
            _reference: &CredentialReference,
        ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Delete,
            })
        }
    }

    impl CredentialStore for UnreadableCredentialStore {
        fn replace(
            &self,
            reference: &CredentialReference,
            secret: CredentialSecret,
        ) -> Result<(), CredentialStoreError> {
            self.delegate.replace(reference, secret)
        }

        fn get(
            &self,
            _reference: &CredentialReference,
        ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
            Err(CredentialStoreError::BackendUnavailable {
                operation: CredentialStoreOperation::Get,
            })
        }

        fn delete(
            &self,
            reference: &CredentialReference,
        ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
            self.delegate.delete(reference)
        }
    }

    impl CredentialStore for RotatingCredentialStore {
        fn replace(
            &self,
            reference: &CredentialReference,
            secret: CredentialSecret,
        ) -> Result<(), CredentialStoreError> {
            self.delegate.replace(reference, secret)
        }

        fn get(
            &self,
            reference: &CredentialReference,
        ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
            let secret = self.delegate.get(reference)?;
            if self.armed.swap(false, Ordering::SeqCst) {
                let mut record = self
                    .storage
                    .load_image_generation_profile(DEFAULT_IMAGE_GENERATION_PROFILE_ID)
                    .unwrap()
                    .unwrap();
                let generation = record.generation;
                record.model_id = "rotated-seedream-model".to_string();
                let outcome = self
                    .storage
                    .compare_and_set_image_generation_profile(
                        DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                        generation,
                        &record,
                    )
                    .unwrap();
                assert!(matches!(
                    outcome,
                    ImageGenerationProfileCompareAndSetOutcome::Updated(_)
                ));
            }
            Ok(secret)
        }

        fn delete(
            &self,
            reference: &CredentialReference,
        ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
            self.delegate.delete(reference)
        }
    }

    fn service() -> (
        ImageGenerationConfigurationService,
        Arc<InMemoryCredentialStore>,
        tempfile::TempDir,
    ) {
        let directory = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&directory.path().join("app.db")).unwrap());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let service = ImageGenerationConfigurationService::new(
            storage,
            credentials.clone() as Arc<dyn CredentialStore>,
        );
        (service, credentials, directory)
    }

    fn update(revision: &str, secret: &str) -> ImageGenerationConfigurationUpdate {
        ImageGenerationConfigurationUpdate {
            expected_revision: revision.to_string(),
            adapter_id: ImageGenerationAdapterId::SmartMlSeedream,
            endpoint_url: "https://zju.smartml.cn/userapi/v1/images/generations".to_string(),
            model_id: "doubao-seedream-4-0-250828".to_string(),
            text_to_image: true,
            image_to_image: false,
            defaults: ImageGenerationDefaults::default(),
            credential_mutation: ImageGenerationCredentialMutation::Replace(
                CredentialSecret::new(secret).unwrap(),
            ),
        }
    }

    #[test]
    fn default_configuration_is_disabled_and_secret_free() {
        let (service, _credentials, _directory) = service();

        let configuration = service.get_configuration().unwrap();

        assert_eq!(configuration.revision, "image-generation:v1:0");
        assert_eq!(configuration.readiness, ImageGenerationReadiness::Disabled);
        assert!(configuration.capabilities.text_to_image);
        assert!(!configuration.capabilities.image_to_image);
        assert_eq!(
            configuration.credential_status,
            ImageGenerationCredentialStatus::Missing
        );
    }

    #[test]
    fn replace_then_enable_publishes_a_ready_unverified_profile() {
        let (service, _credentials, _directory) = service();
        let configured = service
            .update_configuration(update("image-generation:v1:0", "test-secret"))
            .unwrap()
            .configuration;
        assert_eq!(configured.revision, "image-generation:v1:1");
        assert!(!configured.enabled);

        let enabled = service
            .set_enabled(&configured.revision, true)
            .unwrap()
            .configuration;
        assert_eq!(enabled.readiness, ImageGenerationReadiness::ReadyUnverified);
        assert_eq!(enabled.revision, "image-generation:v1:2");
        assert!(enabled.provider_profile().is_ok());
    }

    #[test]
    fn execution_snapshot_never_pairs_a_rotated_profile_with_a_stale_read() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&directory.path().join("app.db")).unwrap());
        let credentials = Arc::new(RotatingCredentialStore {
            delegate: InMemoryCredentialStore::default(),
            storage: Arc::clone(&storage),
            armed: AtomicBool::new(false),
        });
        let service = ImageGenerationConfigurationService::new(
            storage,
            Arc::clone(&credentials) as Arc<dyn CredentialStore>,
        );
        let configured = service
            .update_configuration(update("image-generation:v1:0", "snapshot-secret"))
            .unwrap()
            .configuration;
        service.set_enabled(&configured.revision, true).unwrap();
        credentials.armed.store(true, Ordering::SeqCst);

        let snapshot = service.resolve_execution_snapshot().unwrap();

        assert_eq!(snapshot.profile.model_id, "rotated-seedream-model");
        assert_eq!(
            snapshot
                .credential
                .with_secret_bytes(|bytes| bytes.to_vec()),
            b"snapshot-secret"
        );
        let debug = format!("{snapshot:?}");
        assert!(!debug.contains("snapshot-secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn stale_revision_does_not_replace_the_credential() {
        let (service, credentials, _directory) = service();
        let first = service
            .update_configuration(update("image-generation:v1:0", "first-secret"))
            .unwrap()
            .configuration;

        let error = service
            .update_configuration(update("image-generation:v1:0", "second-secret"))
            .unwrap_err();
        assert_eq!(
            error,
            ImageGenerationConfigurationError::RevisionConflict {
                current_revision: first.revision
            }
        );

        let record = service.load_record().unwrap().unwrap();
        let reference = CredentialReference::parse(record.credential_ref.unwrap()).unwrap();
        let secret = credentials.get(&reference).unwrap().unwrap();
        let value = secret.with_secret_bytes(|bytes| String::from_utf8(bytes.to_vec()).unwrap());
        assert_eq!(value, "first-secret");
    }

    #[test]
    fn enabling_incomplete_configuration_fails_closed() {
        let (service, _credentials, _directory) = service();

        let error = service
            .set_enabled("image-generation:v1:0", true)
            .unwrap_err();

        assert_eq!(error, ImageGenerationConfigurationError::MissingEndpoint);
    }

    #[test]
    fn clear_credential_disables_ready_state_and_queues_safe_cleanup() {
        let (service, _credentials, _directory) = service();
        let configured = service
            .update_configuration(update("image-generation:v1:0", "test-secret"))
            .unwrap()
            .configuration;
        let mut clear = update(&configured.revision, "unused");
        clear.credential_mutation = ImageGenerationCredentialMutation::Clear;

        let cleared = service.update_configuration(clear).unwrap().configuration;

        assert_eq!(
            cleared.credential_status,
            ImageGenerationCredentialStatus::Missing
        );
        assert_eq!(cleared.readiness, ImageGenerationReadiness::Disabled);
        assert!(service
            .storage
            .list_image_generation_credential_cleanup()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn unavailable_native_store_keeps_snapshot_disable_and_clear_recoverable() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&directory.path().join("app.db")).unwrap());
        let credentials = Arc::new(InMemoryCredentialStore::default());
        let setup = ImageGenerationConfigurationService::new(
            Arc::clone(&storage),
            credentials as Arc<dyn CredentialStore>,
        );
        let configured = setup
            .update_configuration(update("image-generation:v1:0", "test-secret"))
            .unwrap()
            .configuration;
        let enabled = setup
            .set_enabled(&configured.revision, true)
            .unwrap()
            .configuration;

        let unavailable =
            ImageGenerationConfigurationService::new(storage, Arc::new(UnavailableCredentialStore));
        let snapshot = unavailable.get_configuration().unwrap();
        assert_eq!(snapshot.revision, enabled.revision);
        assert_eq!(
            snapshot.credential_status,
            ImageGenerationCredentialStatus::Unavailable
        );
        assert_eq!(
            snapshot.readiness,
            ImageGenerationReadiness::CredentialUnavailable
        );
        let disabled = unavailable
            .set_enabled(&snapshot.revision, false)
            .unwrap()
            .configuration;

        assert!(!disabled.enabled);
        assert_eq!(disabled.readiness, ImageGenerationReadiness::Disabled);
        assert_eq!(
            disabled.credential_status,
            ImageGenerationCredentialStatus::Unavailable
        );
        let mut clear = update(&disabled.revision, "unused");
        clear.credential_mutation = ImageGenerationCredentialMutation::Clear;
        let cleared = unavailable
            .update_configuration(clear)
            .unwrap()
            .configuration;
        assert_eq!(
            cleared.credential_status,
            ImageGenerationCredentialStatus::Missing
        );
        assert_eq!(cleared.readiness, ImageGenerationReadiness::Disabled);
        assert_eq!(
            unavailable
                .storage
                .list_image_generation_credential_cleanup()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn replacing_or_clearing_a_credential_does_not_require_reading_the_old_secret() {
        let directory = tempdir().unwrap();
        let storage = Arc::new(StorageService::open(&directory.path().join("app.db")).unwrap());
        let service = ImageGenerationConfigurationService::new(
            storage,
            Arc::new(UnreadableCredentialStore::default()),
        );

        let first = service
            .update_configuration(update("image-generation:v1:0", "first-secret"))
            .unwrap()
            .configuration;
        let snapshot = service.get_configuration().unwrap();
        assert_eq!(snapshot.revision, first.revision);
        assert_eq!(
            snapshot.credential_status,
            ImageGenerationCredentialStatus::Unavailable
        );
        let second = service
            .update_configuration(update(&snapshot.revision, "replacement-secret"))
            .unwrap()
            .configuration;
        let mut clear = update(&second.revision, "unused");
        clear.credential_mutation = ImageGenerationCredentialMutation::Clear;
        let cleared = service.update_configuration(clear).unwrap().configuration;

        assert_eq!(
            cleared.credential_status,
            ImageGenerationCredentialStatus::Missing
        );
        assert_eq!(cleared.readiness, ImageGenerationReadiness::Disabled);
    }

    #[test]
    fn startup_reconciliation_deletes_an_orphaned_staged_credential() {
        let (service, credentials, _directory) = service();
        let reference = CredentialReference::new_opaque();
        service
            .storage
            .stage_image_generation_credential(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                0,
                reference.as_str(),
            )
            .unwrap();
        credentials
            .replace(&reference, CredentialSecret::new("orphan-secret").unwrap())
            .unwrap();

        let report = service.reconcile_credentials().unwrap();

        assert_eq!(report.removed_orphaned_credentials, 1);
        assert!(credentials.get(&reference).unwrap().is_none());
        assert!(service
            .storage
            .list_image_generation_credential_staging()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn startup_reconciliation_keeps_the_credential_referenced_by_a_committed_profile() {
        let (service, credentials, _directory) = service();
        let configured = service
            .update_configuration(update("image-generation:v1:0", "active-secret"))
            .unwrap()
            .configuration;
        let record = service.load_record().unwrap().unwrap();
        let reference = CredentialReference::parse(record.credential_ref.unwrap()).unwrap();
        service
            .storage
            .stage_image_generation_credential(
                DEFAULT_IMAGE_GENERATION_PROFILE_ID,
                configured.generation,
                reference.as_str(),
            )
            .unwrap();

        let report = service.reconcile_credentials().unwrap();

        assert_eq!(report.completed_staging, 1);
        assert!(credentials.get(&reference).unwrap().is_some());
        assert!(service
            .storage
            .list_image_generation_credential_staging()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_http_endpoint_and_disabled_text_to_image() {
        let (service, _credentials, _directory) = service();
        let mut insecure = update("image-generation:v1:0", "secret");
        insecure.endpoint_url = "http://example.com/images/generations".to_string();
        assert_eq!(
            service.update_configuration(insecure).unwrap_err(),
            ImageGenerationConfigurationError::InsecureEndpoint
        );

        let mut credential_bearing_query = update("image-generation:v1:0", "secret");
        credential_bearing_query.endpoint_url =
            "https://example.com/images/generations?api_key=must-not-persist".to_string();
        assert_eq!(
            service
                .update_configuration(credential_bearing_query)
                .unwrap_err(),
            ImageGenerationConfigurationError::InvalidEndpoint
        );

        let mut missing_capability = update("image-generation:v1:0", "secret");
        missing_capability.text_to_image = false;
        assert_eq!(
            service
                .update_configuration(missing_capability)
                .unwrap_err(),
            ImageGenerationConfigurationError::TextToImageRequired
        );

        let whitespace_credential = update("image-generation:v1:0", "secret token");
        assert_eq!(
            service
                .update_configuration(whitespace_credential)
                .unwrap_err(),
            ImageGenerationConfigurationError::InvalidCredential
        );
    }

    #[test]
    fn adapter_transition_can_never_reuse_the_previous_adapter_credential() {
        let mut current = default_record();
        current.adapter_id = "futureAdapter".to_string();
        current.credential_ref = Some(CredentialReference::new_opaque().as_str().to_string());
        let mut keep = update("image-generation:v1:0", "unused");
        keep.credential_mutation = ImageGenerationCredentialMutation::Keep;

        assert_eq!(
            validate_credential_adapter_transition(Some(&current), &keep).unwrap_err(),
            ImageGenerationConfigurationError::CredentialReplacementRequired
        );

        keep.credential_mutation = ImageGenerationCredentialMutation::Replace(
            CredentialSecret::new("new-adapter-secret").unwrap(),
        );
        assert!(validate_credential_adapter_transition(Some(&current), &keep).is_ok());
        keep.credential_mutation = ImageGenerationCredentialMutation::Clear;
        assert!(validate_credential_adapter_transition(Some(&current), &keep).is_ok());
    }
}
