//! Application-facing orchestration for managed Skill installation.
//!
//! Acquisition and store mutation remain separate ports. Local-directory
//! acquisition is the first adapter; future Git, URL, ZIP, or registry
//! adapters must also produce a [`PreparedSkillPackage`] before calling the
//! same prepared-package mutation methods.

use super::acquisition_provenance::SkillInstallationProvenance;
use super::installed::USER_INSTALLED_SKILL_SOURCE_ID;
use super::managed_installer::{
    ManagedSkillInstallOutcome, ManagedSkillInstallRequest, ManagedSkillInstaller,
    ManagedSkillInstallerError, ManagedSkillMutationResult, ManagedSkillUninstallOutcome,
    ManagedSkillUninstallRequest, ManagedSkillUpdateOutcome, ManagedSkillUpdateRequest,
};
use super::managed_store::{InstalledSkillReceipt, ManagedSkillStore, ManagedStoreLoadError};
use super::model::{
    SkillDiagnosticCode, SkillDiagnosticSeverity, SkillId, SkillInstallationId,
    SkillInstallationRevision, SkillRevision, SkillSourceId,
};
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

const USER_SELECTED_DIRECTORY_ORIGIN_REFERENCE: &str = "user-selected-directory";

#[derive(Debug)]
pub struct SkillInstallationService {
    installer: ManagedSkillInstaller,
    store: ManagedSkillStore,
    installed_source_id: SkillSourceId,
}

impl SkillInstallationService {
    /// Configures the application service without touching the filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ManagedSkillInstallerError> {
        let root = root.into();
        let installer = ManagedSkillInstaller::new(root.clone())?;
        let store = ManagedSkillStore::new(root)
            .map_err(|reason| ManagedSkillInstallerError::InvalidStore { reason })?;
        let installed_source_id = SkillSourceId::parse(USER_INSTALLED_SKILL_SOURCE_ID)
            .expect("the built-in installed Skill source id must remain valid");
        Ok(Self {
            installer,
            store,
            installed_source_id,
        })
    }

    /// Acquires one user-selected local directory and installs its validated
    /// byte snapshot under the caller-provided idempotency identity.
    pub fn install_local_directory(
        &self,
        request: &LocalSkillInstallRequest,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let skill_id = self.skill_id(&request.installation_id);
        let package = self.prepare_local(
            SkillInstallationOperation::Install,
            &request.installation_id,
            &skill_id,
            &request.directory,
        )?;
        self.install_prepared(request.installation_id.clone(), package)
    }

    /// Compatibility entry point that acquires a new local snapshot and CASes
    /// on package revision.
    ///
    /// New callers should use [`Self::update_local_directory_exact`]. Package
    /// revisions cannot distinguish an A→B→A lifecycle; this bridge resolves
    /// the current installation revision immediately before the transaction.
    pub fn update_local_directory(
        &self,
        request: &LocalSkillUpdateRequest,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let installation_id =
            self.installed_identity(SkillInstallationOperation::Update, &request.skill_id)?;
        let package = self.prepare_local(
            SkillInstallationOperation::Update,
            &installation_id,
            &request.skill_id,
            &request.directory,
        )?;
        self.update_prepared(installation_id, request.expected_revision.clone(), package)
    }

    /// Exact lifecycle-CAS variant of local-directory update.
    pub fn update_local_directory_exact(
        &self,
        request: &LocalSkillUpdateExactRequest,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let installation_id =
            self.installed_identity(SkillInstallationOperation::Update, &request.skill_id)?;
        let package = self.prepare_local(
            SkillInstallationOperation::Update,
            &installation_id,
            &request.skill_id,
            &request.directory,
        )?;
        let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
        self.update_prepared_exact(
            installation_id,
            request.expected_revision.clone(),
            package,
            provenance,
        )
    }

    /// Removes an installation using its exact lifecycle CAS revision.
    pub fn uninstall_exact(
        &self,
        request: &SkillUninstallExactRequest,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Uninstall;
        let installation_id = self.installed_identity(operation, &request.skill_id)?;
        self.uninstall_installation(
            operation,
            installation_id,
            request.skill_id.clone(),
            request.expected_revision.clone(),
        )
    }

    fn uninstall_installation(
        &self,
        operation: SkillInstallationOperation,
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let installer_request =
            ManagedSkillUninstallRequest::new(installation_id.clone(), expected_revision);
        let result = self
            .installer
            .uninstall(&installer_request)
            .map_err(|source| SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(source),
            })?;
        Ok(Self::uninstall_mutation(
            operation,
            installation_id,
            skill_id,
            result,
        ))
    }

    fn uninstall_mutation(
        operation: SkillInstallationOperation,
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        result: ManagedSkillMutationResult<ManagedSkillUninstallOutcome>,
    ) -> SkillInstallationMutation {
        let outcome = match result.outcome() {
            ManagedSkillUninstallOutcome::Uninstalled => SkillInstallationOutcome::Uninstalled,
            ManagedSkillUninstallOutcome::AlreadyAbsent => SkillInstallationOutcome::AlreadyAbsent,
        };
        SkillInstallationMutation {
            operation,
            outcome,
            installation_id,
            skill_id,
            package_revision: None,
            installation_revision: None,
        }
    }

    /// Commits a package prepared by any acquisition adapter.
    pub fn install_prepared(
        &self,
        installation_id: SkillInstallationId,
        package: PreparedSkillPackage,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
        self.install_prepared_with_provenance(installation_id, package, provenance)
    }

    /// Commits a package with adapter-validated, credential-free provenance.
    pub fn install_prepared_with_provenance(
        &self,
        installation_id: SkillInstallationId,
        package: PreparedSkillPackage,
        provenance: SkillInstallationProvenance,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Install;
        let skill_id = self.skill_id(&installation_id);
        let request = ManagedSkillInstallRequest::with_installation_id_and_provenance(
            installation_id.clone(),
            package,
            provenance,
        );
        let result = self.installer.install(&request).map_err(|source| {
            SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(source),
            }
        })?;
        let committed = result.committed_state().cloned().ok_or_else(|| {
            self.missing_committed_state_error(operation, &installation_id, &skill_id)
        })?;
        let outcome = match result.outcome() {
            ManagedSkillInstallOutcome::Installed => SkillInstallationOutcome::Installed,
            ManagedSkillInstallOutcome::AlreadyInstalled => {
                SkillInstallationOutcome::AlreadyInstalled
            }
        };
        Ok(SkillInstallationMutation {
            operation,
            outcome,
            installation_id,
            skill_id,
            package_revision: Some(committed.package_revision().clone()),
            installation_revision: Some(committed.installation_revision().clone()),
        })
    }

    /// Compatibility entry point for acquisition adapters that still CAS on
    /// package revision. New adapters should use [`Self::update_prepared_exact`].
    pub fn update_prepared(
        &self,
        installation_id: SkillInstallationId,
        expected_revision: SkillRevision,
        package: PreparedSkillPackage,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Update;
        let skill_id = self.skill_id(&installation_id);
        let provenance = SkillInstallationProvenance::from_legacy_origin(package.origin());
        let receipt = self
            .load_legacy_receipt(operation, &installation_id, &skill_id)?
            .ok_or_else(|| SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(ManagedSkillInstallerError::InstallationNotFound {
                    installation_id: installation_id.clone(),
                }),
            })?;
        if receipt.is_legacy_v1()
            || receipt.package.format_version != package.format_version()
            || receipt.package.revision != *package.revision()
            || receipt.provenance != provenance
        {
            self.validate_legacy_package_revision(
                operation,
                &receipt,
                &skill_id,
                &expected_revision,
            )?;
        }
        let expected_installation_revision = receipt.installation_revision;
        self.update_prepared_exact(
            installation_id,
            expected_installation_revision,
            package,
            provenance,
        )
    }

    /// Updates a managed installation using exact lifecycle CAS and explicit
    /// adapter-validated provenance.
    pub fn update_prepared_exact(
        &self,
        installation_id: SkillInstallationId,
        expected_revision: SkillInstallationRevision,
        package: PreparedSkillPackage,
        provenance: SkillInstallationProvenance,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Update;
        let skill_id = self.skill_id(&installation_id);
        let request = ManagedSkillUpdateRequest::with_provenance(
            installation_id.clone(),
            expected_revision,
            package,
            provenance,
        );
        let result = self.installer.update(&request).map_err(|source| {
            SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(source),
            }
        })?;
        let committed = result.committed_state().cloned().ok_or_else(|| {
            self.missing_committed_state_error(operation, &installation_id, &skill_id)
        })?;
        let outcome = match result.outcome() {
            ManagedSkillUpdateOutcome::Updated => SkillInstallationOutcome::Updated,
            ManagedSkillUpdateOutcome::AlreadyCurrent => SkillInstallationOutcome::AlreadyCurrent,
        };
        Ok(SkillInstallationMutation {
            operation,
            outcome,
            installation_id,
            skill_id,
            package_revision: Some(committed.package_revision().clone()),
            installation_revision: Some(committed.installation_revision().clone()),
        })
    }

    /// Reads one normalized receipt record without loading package bytes.
    pub fn read_installed_skill(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<Option<InstalledSkillRecord>, SkillInstallationInventoryError> {
        match self.store.load_receipt(installation_id) {
            Ok(receipt) => Ok(Some(self.installed_record(receipt))),
            Err(ManagedStoreLoadError::NotFound) => Ok(None),
            Err(ManagedStoreLoadError::Invalid(issue))
            | Err(ManagedStoreLoadError::Unavailable(issue)) => {
                Err(SkillInstallationInventoryError::new(issue.message))
            }
        }
    }

    /// Lists normalized receipt records and isolated per-receipt diagnostics.
    pub fn list_installed_skills(
        &self,
    ) -> Result<InstalledSkillInventory, SkillInstallationInventoryError> {
        let index = self
            .store
            .scan_receipts()
            .map_err(|error| SkillInstallationInventoryError::new(error.to_string()))?;
        Ok(InstalledSkillInventory {
            records: index
                .receipts
                .into_iter()
                .map(|receipt| self.installed_record(receipt))
                .collect(),
            issues: index
                .issues
                .into_iter()
                .map(|issue| InstalledSkillRecordIssue {
                    code: issue.code,
                    severity: issue.severity,
                    location: issue.location,
                    message: issue.message,
                })
                .collect(),
            truncated: index.truncated,
        })
    }

    fn installed_record(&self, receipt: InstalledSkillReceipt) -> InstalledSkillRecord {
        InstalledSkillRecord {
            skill_id: self.skill_id(&receipt.installation_id),
            installation_id: receipt.installation_id,
            receipt_schema_version: receipt.receipt_schema_version,
            generation: receipt.generation,
            package_format_version: receipt.package.format_version,
            package_revision: receipt.package.revision,
            provenance: receipt.provenance,
            installation_revision: receipt.installation_revision,
            installed_at_unix_ms: receipt.installed_at_unix_ms,
            updated_at_unix_ms: receipt.updated_at_unix_ms,
        }
    }

    fn load_legacy_receipt(
        &self,
        operation: SkillInstallationOperation,
        installation_id: &SkillInstallationId,
        skill_id: &SkillId,
    ) -> Result<Option<InstalledSkillReceipt>, SkillInstallationServiceError> {
        match self.store.load_receipt(installation_id) {
            Ok(receipt) => Ok(Some(receipt)),
            Err(ManagedStoreLoadError::NotFound) => Ok(None),
            Err(ManagedStoreLoadError::Invalid(issue)) => {
                Err(SkillInstallationServiceError::Installer {
                    operation,
                    installation_id: installation_id.clone(),
                    skill_id: skill_id.clone(),
                    source: Box::new(ManagedSkillInstallerError::StoreCorrupt {
                        reason: issue.message,
                    }),
                })
            }
            Err(ManagedStoreLoadError::Unavailable(issue)) => {
                Err(SkillInstallationServiceError::Installer {
                    operation,
                    installation_id: installation_id.clone(),
                    skill_id: skill_id.clone(),
                    source: Box::new(ManagedSkillInstallerError::Io {
                        operation: "read managed Skill installation receipt".to_string(),
                        reason: issue.message,
                    }),
                })
            }
        }
    }

    fn validate_legacy_package_revision(
        &self,
        operation: SkillInstallationOperation,
        receipt: &InstalledSkillReceipt,
        skill_id: &SkillId,
        expected_package_revision: &SkillRevision,
    ) -> Result<(), SkillInstallationServiceError> {
        if receipt.package.revision != *expected_package_revision {
            return Err(SkillInstallationServiceError::LegacyRevisionConflict {
                operation,
                installation_id: receipt.installation_id.clone(),
                skill_id: skill_id.clone(),
                expected_revision: expected_package_revision.clone(),
                actual_revision: receipt.package.revision.clone(),
            });
        }
        Ok(())
    }

    fn missing_committed_state_error(
        &self,
        operation: SkillInstallationOperation,
        installation_id: &SkillInstallationId,
        skill_id: &SkillId,
    ) -> SkillInstallationServiceError {
        SkillInstallationServiceError::Installer {
            operation,
            installation_id: installation_id.clone(),
            skill_id: skill_id.clone(),
            source: Box::new(ManagedSkillInstallerError::StoreCorrupt {
                reason: "managed installer returned no committed receipt state".to_string(),
            }),
        }
    }

    fn prepare_local(
        &self,
        operation: SkillInstallationOperation,
        installation_id: &SkillInstallationId,
        skill_id: &SkillId,
        directory: &Path,
    ) -> Result<PreparedSkillPackage, SkillInstallationServiceError> {
        PreparedSkillPackage::from_local_directory(
            directory,
            USER_SELECTED_DIRECTORY_ORIGIN_REFERENCE,
        )
        .map_err(|source| SkillInstallationServiceError::Preparation {
            operation,
            installation_id: installation_id.clone(),
            skill_id: skill_id.clone(),
            source: Box::new(source),
        })
    }

    fn installed_identity(
        &self,
        operation: SkillInstallationOperation,
        skill_id: &SkillId,
    ) -> Result<SkillInstallationId, SkillInstallationServiceError> {
        if skill_id.source_id() != &self.installed_source_id {
            return Err(SkillInstallationServiceError::InvalidInstalledSkill {
                operation,
                skill_id: skill_id.clone(),
                reason: format!(
                    "Skill `{skill_id}` does not belong to the user-installed source `{}`",
                    self.installed_source_id
                ),
            });
        }
        SkillInstallationId::parse(skill_id.local_id()).map_err(|error| {
            SkillInstallationServiceError::InvalidInstalledSkill {
                operation,
                skill_id: skill_id.clone(),
                reason: format!("invalid installed Skill identity: {error}"),
            }
        })
    }

    fn skill_id(&self, installation_id: &SkillInstallationId) -> SkillId {
        SkillId::from_parts(self.installed_source_id.clone(), installation_id.as_str())
            .expect("a canonical installation id must form a valid installed Skill id")
    }
}

#[derive(Debug, Clone)]
pub struct LocalSkillInstallRequest {
    installation_id: SkillInstallationId,
    directory: PathBuf,
}

impl LocalSkillInstallRequest {
    pub fn new(installation_id: SkillInstallationId, directory: impl Into<PathBuf>) -> Self {
        Self {
            installation_id,
            directory: directory.into(),
        }
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug, Clone)]
pub struct LocalSkillUpdateRequest {
    skill_id: SkillId,
    expected_revision: SkillRevision,
    directory: PathBuf,
}

impl LocalSkillUpdateRequest {
    pub fn new(
        skill_id: SkillId,
        expected_revision: SkillRevision,
        directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            skill_id,
            expected_revision,
            directory: directory.into(),
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
        &self.expected_revision
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug, Clone)]
pub struct LocalSkillUpdateExactRequest {
    skill_id: SkillId,
    expected_revision: SkillInstallationRevision,
    directory: PathBuf,
}

impl LocalSkillUpdateExactRequest {
    pub fn new(
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
        directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            skill_id,
            expected_revision,
            directory: directory.into(),
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillInstallationRevision {
        &self.expected_revision
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug, Clone)]
pub struct SkillUninstallExactRequest {
    skill_id: SkillId,
    expected_revision: SkillInstallationRevision,
}

impl SkillUninstallExactRequest {
    pub fn new(skill_id: SkillId, expected_revision: SkillInstallationRevision) -> Self {
        Self {
            skill_id,
            expected_revision,
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillInstallationRevision {
        &self.expected_revision
    }
}

/// Immutable, credential-safe projection of one normalized installation receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkillRecord {
    installation_id: SkillInstallationId,
    skill_id: SkillId,
    receipt_schema_version: u32,
    generation: u64,
    package_format_version: u32,
    package_revision: SkillRevision,
    provenance: SkillInstallationProvenance,
    installation_revision: SkillInstallationRevision,
    installed_at_unix_ms: u64,
    updated_at_unix_ms: u64,
}

impl InstalledSkillRecord {
    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn receipt_schema_version(&self) -> u32 {
        self.receipt_schema_version
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn package_format_version(&self) -> u32 {
        self.package_format_version
    }

    pub fn package_revision(&self) -> &SkillRevision {
        &self.package_revision
    }

    pub fn provenance(&self) -> &SkillInstallationProvenance {
        &self.provenance
    }

    pub fn installation_revision(&self) -> &SkillInstallationRevision {
        &self.installation_revision
    }

    pub fn installed_at_unix_ms(&self) -> u64 {
        self.installed_at_unix_ms
    }

    pub fn updated_at_unix_ms(&self) -> u64 {
        self.updated_at_unix_ms
    }

    pub fn is_legacy(&self) -> bool {
        self.receipt_schema_version == 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkillRecordIssue {
    code: SkillDiagnosticCode,
    severity: SkillDiagnosticSeverity,
    location: String,
    message: String,
}

impl InstalledSkillRecordIssue {
    pub fn code(&self) -> SkillDiagnosticCode {
        self.code
    }

    pub fn severity(&self) -> SkillDiagnosticSeverity {
        self.severity
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkillInventory {
    records: Vec<InstalledSkillRecord>,
    issues: Vec<InstalledSkillRecordIssue>,
    truncated: bool,
}

impl InstalledSkillInventory {
    pub fn records(&self) -> &[InstalledSkillRecord] {
        &self.records
    }

    pub fn issues(&self) -> &[InstalledSkillRecordIssue] {
        &self.issues
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillInstallationInventoryError {
    reason: String,
}

impl SkillInstallationInventoryError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillInstallationInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillInstallationInventoryError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillInstallationOperation {
    Install,
    Update,
    Uninstall,
}

impl SkillInstallationOperation {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillInstallationOutcome {
    Installed,
    AlreadyInstalled,
    Updated,
    AlreadyCurrent,
    Uninstalled,
    AlreadyAbsent,
}

impl SkillInstallationOutcome {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::AlreadyInstalled => "alreadyInstalled",
            Self::Updated => "updated",
            Self::AlreadyCurrent => "alreadyCurrent",
            Self::Uninstalled => "uninstalled",
            Self::AlreadyAbsent => "alreadyAbsent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationMutation {
    operation: SkillInstallationOperation,
    outcome: SkillInstallationOutcome,
    installation_id: SkillInstallationId,
    skill_id: SkillId,
    package_revision: Option<SkillRevision>,
    installation_revision: Option<SkillInstallationRevision>,
}

impl SkillInstallationMutation {
    pub fn operation(&self) -> SkillInstallationOperation {
        self.operation
    }

    pub fn outcome(&self) -> SkillInstallationOutcome {
        self.outcome
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn revision(&self) -> Option<&SkillRevision> {
        self.package_revision()
    }

    pub fn package_revision(&self) -> Option<&SkillRevision> {
        self.package_revision.as_ref()
    }

    pub fn installation_revision(&self) -> Option<&SkillInstallationRevision> {
        self.installation_revision.as_ref()
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum SkillInstallationServiceError {
    InvalidInstalledSkill {
        operation: SkillInstallationOperation,
        skill_id: SkillId,
        reason: String,
    },
    Preparation {
        operation: SkillInstallationOperation,
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        source: Box<SkillPackagePreparationError>,
    },
    /// Package-revision conflict for the current local-update acquisition lane.
    LegacyRevisionConflict {
        operation: SkillInstallationOperation,
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        expected_revision: SkillRevision,
        actual_revision: SkillRevision,
    },
    Installer {
        operation: SkillInstallationOperation,
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        source: Box<ManagedSkillInstallerError>,
    },
}

impl SkillInstallationServiceError {
    pub fn operation(&self) -> SkillInstallationOperation {
        match self {
            Self::InvalidInstalledSkill { operation, .. }
            | Self::Preparation { operation, .. }
            | Self::LegacyRevisionConflict { operation, .. }
            | Self::Installer { operation, .. } => *operation,
        }
    }

    pub fn installation_id(&self) -> Option<&SkillInstallationId> {
        match self {
            Self::InvalidInstalledSkill { .. } => None,
            Self::Preparation {
                installation_id, ..
            }
            | Self::LegacyRevisionConflict {
                installation_id, ..
            }
            | Self::Installer {
                installation_id, ..
            } => Some(installation_id),
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        match self {
            Self::InvalidInstalledSkill { skill_id, .. }
            | Self::Preparation { skill_id, .. }
            | Self::LegacyRevisionConflict { skill_id, .. }
            | Self::Installer { skill_id, .. } => skill_id,
        }
    }

    pub fn preparation_error(&self) -> Option<&SkillPackagePreparationError> {
        match self {
            Self::Preparation { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }

    pub fn installer_error(&self) -> Option<&ManagedSkillInstallerError> {
        match self {
            Self::Installer { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl fmt::Display for SkillInstallationServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInstalledSkill { reason, .. } => reason.fmt(formatter),
            Self::Preparation { source, .. } => source.fmt(formatter),
            Self::LegacyRevisionConflict {
                skill_id,
                expected_revision,
                actual_revision,
                ..
            } => write!(
                formatter,
                "Skill `{skill_id}` package revision conflict: expected `{expected_revision}`, actual `{actual_revision}`"
            ),
            Self::Installer { source, .. } => source.fmt(formatter),
        }
    }
}

impl Error for SkillInstallationServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidInstalledSkill { .. } => None,
            Self::Preparation { source, .. } => Some(source.as_ref()),
            Self::LegacyRevisionConflict { .. } => None,
            Self::Installer { source, .. } => Some(source.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillErrorCode, SkillInstallationAuthority, SkillInstallationRefresh, SkillSelection,
        SkillsService,
    };
    use std::fs;
    use tempfile::tempdir;

    const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";

    fn write_local(directory: &Path, marker: &str) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join("SKILL.md"),
            format!(
                "---\nname: local-auditor\ndescription: Local lifecycle fixture.\n---\n# Instructions\n{marker}\n"
            ),
        )
        .unwrap();
    }

    fn provenance(authority: &str, refresh: Option<&str>) -> SkillInstallationProvenance {
        SkillInstallationProvenance::new(
            SkillInstallationAuthority::new("fixture", 1, authority).unwrap(),
            refresh.map(|payload| SkillInstallationRefresh::new("fixture", 1, payload).unwrap()),
        )
    }

    #[test]
    fn local_directory_lifecycle_is_visible_and_activatable_through_the_real_source() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let local = fixture.path().join("local-skill");
        write_local(&local, "VERSION_ONE");
        let service = SkillInstallationService::new(&store).unwrap();
        let reader = SkillsService::new().with_installed_source(&store).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();

        let install = service
            .install_local_directory(&LocalSkillInstallRequest::new(
                installation_id.clone(),
                &local,
            ))
            .unwrap();
        assert_eq!(install.outcome(), SkillInstallationOutcome::Installed);
        let first = reader.list().unwrap().skills()[0].clone();
        assert_eq!(first.id(), install.skill_id());
        let first_selection = first.selection();
        let activated = reader
            .activate(std::slice::from_ref(&first_selection))
            .unwrap();
        assert!(activated.skills()[0].instructions().contains("VERSION_ONE"));

        write_local(&local, "VERSION_TWO");
        let update_request =
            LocalSkillUpdateRequest::new(first.id().clone(), first.revision().clone(), &local);
        let update = service.update_local_directory(&update_request).unwrap();
        assert_eq!(update.outcome(), SkillInstallationOutcome::Updated);
        assert_eq!(
            service
                .update_local_directory(&update_request)
                .unwrap()
                .outcome(),
            SkillInstallationOutcome::AlreadyCurrent
        );
        let stale = reader.activate(&[first_selection]).unwrap_err();
        assert_eq!(stale.code(), SkillErrorCode::Stale);
        let second = reader.list().unwrap().skills()[0].clone();
        assert_eq!(second.revision(), update.revision().unwrap());
        let activated = reader.activate(&[second.selection()]).unwrap();
        assert!(activated.skills()[0].instructions().contains("VERSION_TWO"));

        let installation_revision = service
            .read_installed_skill(&installation_id)
            .unwrap()
            .unwrap()
            .installation_revision()
            .clone();
        let uninstall_request =
            SkillUninstallExactRequest::new(second.id().clone(), installation_revision);
        let uninstall = service.uninstall_exact(&uninstall_request).unwrap();
        assert_eq!(uninstall.outcome(), SkillInstallationOutcome::Uninstalled);
        assert_eq!(
            service
                .uninstall_exact(&uninstall_request)
                .unwrap()
                .outcome(),
            SkillInstallationOutcome::AlreadyAbsent
        );
        assert!(reader.list().unwrap().skills().is_empty());
        let not_found = reader
            .resolve(&SkillSelection::new(
                second.id().clone(),
                second.revision().clone(),
            ))
            .unwrap_err();
        assert_eq!(not_found.code(), SkillErrorCode::NotFound);
    }

    #[test]
    fn retries_converge_and_non_installed_skill_ids_are_rejected() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let local = fixture.path().join("local-skill");
        write_local(&local, "STABLE");
        let service = SkillInstallationService::new(&store).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let request = LocalSkillInstallRequest::new(installation_id, &local);

        assert_eq!(
            service.install_local_directory(&request).unwrap().outcome(),
            SkillInstallationOutcome::Installed
        );
        assert_eq!(
            service.install_local_directory(&request).unwrap().outcome(),
            SkillInstallationOutcome::AlreadyInstalled
        );

        let workspace_id = SkillId::parse("workspace:project:local-auditor").unwrap();
        let error = service
            .uninstall_exact(&SkillUninstallExactRequest::new(
                workspace_id,
                SkillInstallationRevision::parse(format!(
                    "skill-installation-sha256-v1:{}",
                    "a".repeat(64)
                ))
                .unwrap(),
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            SkillInstallationServiceError::InvalidInstalledSkill { .. }
        ));
    }

    #[test]
    fn local_acquisition_failures_keep_operation_and_identity_context() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let missing = fixture.path().join("missing");
        let service = SkillInstallationService::new(&store).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();

        let error = service
            .install_local_directory(&LocalSkillInstallRequest::new(
                installation_id.clone(),
                missing,
            ))
            .unwrap_err();

        assert_eq!(error.operation(), SkillInstallationOperation::Install);
        assert_eq!(error.installation_id(), Some(&installation_id));
        assert_eq!(
            error.preparation_error().unwrap().code(),
            crate::skills::SkillDiagnosticCode::InvalidRoot
        );
        assert!(error.installer_error().is_none());
    }

    #[test]
    fn exact_lifecycle_and_inventory_use_authoritative_receipt_revisions() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let local = fixture.path().join("local-skill");
        write_local(&local, "EXACT_LIFECYCLE");
        let package = PreparedSkillPackage::from_local_directory(
            &local,
            USER_SELECTED_DIRECTORY_ORIGIN_REFERENCE,
        )
        .unwrap();
        let service = SkillInstallationService::new(&store).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        let first_provenance = provenance("immutable-a", Some("refresh-a"));

        let install = service
            .install_prepared_with_provenance(
                installation_id.clone(),
                package.clone(),
                first_provenance.clone(),
            )
            .unwrap();
        assert_eq!(install.outcome(), SkillInstallationOutcome::Installed);
        let first = service
            .read_installed_skill(&installation_id)
            .unwrap()
            .unwrap();
        assert_eq!(first.receipt_schema_version(), 2);
        assert_eq!(first.generation(), 1);
        assert_eq!(
            first.package_revision(),
            install.package_revision().unwrap()
        );
        assert_eq!(
            first.installation_revision(),
            install.installation_revision().unwrap()
        );
        assert_eq!(first.provenance(), &first_provenance);
        assert!(!first.is_legacy());
        let inventory = service.list_installed_skills().unwrap();
        assert_eq!(inventory.records(), std::slice::from_ref(&first));
        assert!(inventory.issues().is_empty());
        assert!(!inventory.truncated());

        let installed_at = first.installed_at_unix_ms();
        let first_updated_at = first.updated_at_unix_ms();
        let first_installation_revision = first.installation_revision().clone();
        let secret_canary = "credential-canary-must-not-leak";
        let second_provenance = provenance("immutable-b", Some(secret_canary));
        let update = service
            .update_prepared_exact(
                installation_id.clone(),
                first_installation_revision.clone(),
                package.clone(),
                second_provenance.clone(),
            )
            .unwrap();
        assert_eq!(update.outcome(), SkillInstallationOutcome::Updated);
        assert_eq!(update.package_revision(), Some(first.package_revision()));
        let second = service
            .read_installed_skill(&installation_id)
            .unwrap()
            .unwrap();
        assert_eq!(second.generation(), 2);
        assert_eq!(second.package_revision(), first.package_revision());
        assert_ne!(second.installation_revision(), &first_installation_revision);
        assert_eq!(second.provenance(), &second_provenance);
        assert_eq!(second.installed_at_unix_ms(), installed_at);
        assert!(second.updated_at_unix_ms() >= first_updated_at);
        assert!(!format!("{second:?}").contains(secret_canary));

        let retry = service
            .update_prepared_exact(
                installation_id.clone(),
                first_installation_revision.clone(),
                package.clone(),
                second_provenance,
            )
            .unwrap();
        assert_eq!(retry.outcome(), SkillInstallationOutcome::AlreadyCurrent);
        let retried = service
            .read_installed_skill(&installation_id)
            .unwrap()
            .unwrap();
        assert_eq!(retried.generation(), second.generation());
        assert_eq!(
            retried.installation_revision(),
            second.installation_revision()
        );
        assert_eq!(retried.updated_at_unix_ms(), second.updated_at_unix_ms());

        let stale = service
            .update_prepared_exact(
                installation_id.clone(),
                first_installation_revision,
                package,
                provenance("immutable-c", None),
            )
            .unwrap_err();
        assert_eq!(
            stale.installer_error().unwrap().code(),
            super::super::managed_installer::ManagedSkillInstallerErrorCode::RevisionConflict
        );
        let uninstall = service
            .uninstall_exact(&SkillUninstallExactRequest::new(
                second.skill_id().clone(),
                second.installation_revision().clone(),
            ))
            .unwrap();
        assert_eq!(uninstall.outcome(), SkillInstallationOutcome::Uninstalled);
        assert_eq!(
            service
                .uninstall_exact(&SkillUninstallExactRequest::new(
                    second.skill_id().clone(),
                    second.installation_revision().clone(),
                ))
                .unwrap()
                .outcome(),
            SkillInstallationOutcome::AlreadyAbsent
        );
        assert!(service
            .read_installed_skill(&installation_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn inventory_isolates_invalid_receipts_while_preserving_valid_records() {
        let fixture = tempdir().unwrap();
        let store = fixture.path().join("store");
        let local = fixture.path().join("local-skill");
        write_local(&local, "VALID");
        let service = SkillInstallationService::new(&store).unwrap();
        let installation_id = SkillInstallationId::parse(INSTALLATION_ID).unwrap();
        service
            .install_local_directory(&LocalSkillInstallRequest::new(
                installation_id.clone(),
                &local,
            ))
            .unwrap();
        let invalid_id =
            SkillInstallationId::parse("11111111-2222-4333-8444-555555555555").unwrap();
        fs::write(
            store
                .join(super::super::managed_store::INSTALLATIONS_DIRECTORY)
                .join(format!("{invalid_id}.json")),
            b"{not-json",
        )
        .unwrap();

        let inventory = service.list_installed_skills().unwrap();
        assert_eq!(inventory.records().len(), 1);
        assert_eq!(inventory.records()[0].installation_id(), &installation_id);
        assert_eq!(inventory.issues().len(), 1);
        assert_eq!(
            inventory.issues()[0].code(),
            SkillDiagnosticCode::InvalidInstallationReceipt
        );
        assert!(service.read_installed_skill(&invalid_id).is_err());
    }
}
