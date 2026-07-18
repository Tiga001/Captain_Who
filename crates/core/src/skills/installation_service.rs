//! Application-facing orchestration for managed Skill installation.
//!
//! Acquisition and store mutation remain separate ports. Local-directory
//! acquisition is the first adapter; future Git, URL, ZIP, or registry
//! adapters must also produce a [`PreparedSkillPackage`] before calling the
//! same prepared-package mutation methods.

use super::installed::USER_INSTALLED_SKILL_SOURCE_ID;
use super::managed_installer::{
    ManagedSkillInstallOutcome, ManagedSkillInstallRequest, ManagedSkillInstaller,
    ManagedSkillInstallerError, ManagedSkillUninstallOutcome, ManagedSkillUninstallRequest,
    ManagedSkillUpdateOutcome, ManagedSkillUpdateRequest,
};
use super::model::{SkillId, SkillInstallationId, SkillRevision, SkillSourceId};
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

const USER_SELECTED_DIRECTORY_ORIGIN_REFERENCE: &str = "user-selected-directory";

#[derive(Debug)]
pub struct SkillInstallationService {
    installer: ManagedSkillInstaller,
    installed_source_id: SkillSourceId,
}

impl SkillInstallationService {
    /// Configures the application service without touching the filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, ManagedSkillInstallerError> {
        let installer = ManagedSkillInstaller::new(root)?;
        let installed_source_id = SkillSourceId::parse(USER_INSTALLED_SKILL_SOURCE_ID)
            .expect("the built-in installed Skill source id must remain valid");
        Ok(Self {
            installer,
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

    /// Acquires a new local snapshot and updates one installed Skill using
    /// revision compare-and-swap semantics.
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

    /// Removes the receipt for one installed Skill. Immutable package objects
    /// remain available for later safe garbage collection.
    pub fn uninstall(
        &self,
        request: &SkillUninstallRequest,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Uninstall;
        let installation_id = self.installed_identity(operation, &request.skill_id)?;
        let installer_request = ManagedSkillUninstallRequest::new(
            installation_id.clone(),
            request.expected_revision.clone(),
        );
        let outcome = self
            .installer
            .uninstall(&installer_request)
            .map_err(|source| SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: request.skill_id.clone(),
                source: Box::new(source),
            })?;
        let outcome = match outcome {
            ManagedSkillUninstallOutcome::Uninstalled => SkillInstallationOutcome::Uninstalled,
            ManagedSkillUninstallOutcome::AlreadyAbsent => SkillInstallationOutcome::AlreadyAbsent,
        };
        Ok(SkillInstallationMutation {
            operation,
            outcome,
            installation_id,
            skill_id: request.skill_id.clone(),
            revision: None,
        })
    }

    /// Commits a package prepared by any acquisition adapter.
    pub fn install_prepared(
        &self,
        installation_id: SkillInstallationId,
        package: PreparedSkillPackage,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Install;
        let skill_id = self.skill_id(&installation_id);
        let revision = package.revision().clone();
        let request =
            ManagedSkillInstallRequest::with_installation_id(installation_id.clone(), package);
        let outcome = self.installer.install(&request).map_err(|source| {
            SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(source),
            }
        })?;
        let outcome = match outcome {
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
            revision: Some(revision),
        })
    }

    /// Updates an installation using a package prepared by any acquisition
    /// adapter.
    pub fn update_prepared(
        &self,
        installation_id: SkillInstallationId,
        expected_revision: SkillRevision,
        package: PreparedSkillPackage,
    ) -> Result<SkillInstallationMutation, SkillInstallationServiceError> {
        let operation = SkillInstallationOperation::Update;
        let skill_id = self.skill_id(&installation_id);
        let revision = package.revision().clone();
        let request =
            ManagedSkillUpdateRequest::new(installation_id.clone(), expected_revision, package);
        let outcome = self.installer.update(&request).map_err(|source| {
            SkillInstallationServiceError::Installer {
                operation,
                installation_id: installation_id.clone(),
                skill_id: skill_id.clone(),
                source: Box::new(source),
            }
        })?;
        let outcome = match outcome {
            ManagedSkillUpdateOutcome::Updated => SkillInstallationOutcome::Updated,
            ManagedSkillUpdateOutcome::AlreadyCurrent => SkillInstallationOutcome::AlreadyCurrent,
        };
        Ok(SkillInstallationMutation {
            operation,
            outcome,
            installation_id,
            skill_id,
            revision: Some(revision),
        })
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
pub struct SkillUninstallRequest {
    skill_id: SkillId,
    expected_revision: SkillRevision,
}

impl SkillUninstallRequest {
    pub fn new(skill_id: SkillId, expected_revision: SkillRevision) -> Self {
        Self {
            skill_id,
            expected_revision,
        }
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
        &self.expected_revision
    }
}

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
    revision: Option<SkillRevision>,
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
        self.revision.as_ref()
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
            | Self::Installer { operation, .. } => *operation,
        }
    }

    pub fn installation_id(&self) -> Option<&SkillInstallationId> {
        match self {
            Self::InvalidInstalledSkill { .. } => None,
            Self::Preparation {
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
            Self::Installer { source, .. } => source.fmt(formatter),
        }
    }
}

impl Error for SkillInstallationServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidInstalledSkill { .. } => None,
            Self::Preparation { source, .. } => Some(source.as_ref()),
            Self::Installer { source, .. } => Some(source.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillErrorCode, SkillSelection, SkillsService};
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
            .install_local_directory(&LocalSkillInstallRequest::new(installation_id, &local))
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

        let uninstall_request =
            SkillUninstallRequest::new(second.id().clone(), second.revision().clone());
        let uninstall = service.uninstall(&uninstall_request).unwrap();
        assert_eq!(uninstall.outcome(), SkillInstallationOutcome::Uninstalled);
        assert_eq!(
            service.uninstall(&uninstall_request).unwrap().outcome(),
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
            .uninstall(&SkillUninstallRequest::new(
                workspace_id,
                SkillRevision::parse("revision").unwrap(),
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
}
