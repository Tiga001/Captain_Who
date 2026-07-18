//! Defensive, read-only access to the application-managed Skill store.
//!
//! Managed packages are immutable content-addressed trees below `packages/`;
//! installation receipts live at `installations/<canonical-lower-uuid>.json`.
//! Receipt schema v1 remains read-only compatible, while every writer emits
//! schema v2 with a generation-bound installation revision and typed acquisition
//! provenance. A live receipt is the sole activation marker; a durable retired
//! identity takes precedence over a live receipt if crash recovery exposes
//! both phases of an uninstall transaction. Readers never write or repair.
//!
//! A future writer must fully publish an immutable package first and atomically
//! replace the receipt last. Old packages can be garbage-collected only after
//! receipt removal/replacement and with already-resolved snapshots in mind.

use super::acquisition_provenance::{
    installation_revision, SkillInstallationAuthority, SkillInstallationProvenance,
    SkillInstallationRefresh,
};
use super::digest::{
    package_file_digest, package_revision, PACKAGE_REVISION_PREFIX, PACKAGE_REVISION_V2_PREFIX,
    PACKAGE_REVISION_V3_PREFIX,
};
use super::model::{
    SkillDiagnosticCode, SkillDiagnosticSeverity, SkillInstallationId, SkillInstallationRevision,
    SkillResourceIndex, SkillRevision, SKILL_PACKAGE_FORMAT_VERSION,
    SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::origin::SkillPackageOrigin;
use super::package::{
    PackageManifest, PackageManifestEntry, PackageValidationError, MAX_SKILL_PACKAGE_BYTES,
    MAX_SKILL_PACKAGE_MANIFEST_BYTES, MAX_SKILL_RESOURCE_FILE_BYTES, PACKAGE_MANIFEST_FILE,
};
use super::workspace::{
    is_symlink_or_reparse, metadata_if_present, percent_encode, read_bounded_verified,
    verify_plain_directory, BoundedReadError, ByteBudget, MAX_SKILL_FILE_BYTES, SKILL_FILE_NAME,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const INSTALLATIONS_DIRECTORY: &str = "installations";
pub(super) const RETIRED_INSTALLATIONS_DIRECTORY: &str = "retired-installations";
pub(super) const PACKAGES_DIRECTORY: &str = "packages";
pub(super) const PACKAGE_V1_DIRECTORY: &str = "v1";
pub(super) const PACKAGE_V2_DIRECTORY: &str = "v2";
pub(super) const PACKAGE_V3_DIRECTORY: &str = "v3";
const RECEIPT_EXTENSION: &str = "json";
const RECEIPT_SCHEMA_VERSION_V1: u32 = 1;
const RECEIPT_SCHEMA_VERSION_V2: u32 = 2;
pub(super) const MAX_LIVE_INSTALLATIONS: usize = 2_000;
pub(super) const MAX_INSTALLATION_DIRECTORY_ENTRIES: usize = 4_096;
pub(super) const MAX_RETIRED_INSTALLATION_ENTRIES: usize = 100_000;
const MAX_RECEIPT_BYTES: usize = 64 * 1024;
const MAX_RECEIPT_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_PACKAGE_CATALOG_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_MANAGED_DIRECTORY_ENTRIES: usize = 100_000;

#[derive(Debug, Clone)]
pub(super) struct ManagedSkillStore {
    root: PathBuf,
    limits: ManagedStoreLimits,
}

#[derive(Debug, Clone, Copy)]
struct ManagedStoreLimits {
    max_live_installations: usize,
    max_installation_directory_entries: usize,
    max_receipt_bytes: usize,
    max_receipt_catalog_bytes: usize,
    max_package_catalog_bytes: usize,
}

impl Default for ManagedStoreLimits {
    fn default() -> Self {
        Self {
            max_live_installations: MAX_LIVE_INSTALLATIONS,
            max_installation_directory_entries: MAX_INSTALLATION_DIRECTORY_ENTRIES,
            max_receipt_bytes: MAX_RECEIPT_BYTES,
            max_receipt_catalog_bytes: MAX_RECEIPT_CATALOG_BYTES,
            max_package_catalog_bytes: MAX_PACKAGE_CATALOG_BYTES,
        }
    }
}

impl ManagedSkillStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        if !root.is_absolute() {
            return Err("managed Skill store root must be an absolute path".to_string());
        }
        Ok(Self {
            root,
            limits: ManagedStoreLimits::default(),
        })
    }

    pub fn package_catalog_budget(&self) -> ByteBudget {
        ByteBudget::new(self.limits.max_package_catalog_bytes)
    }

    #[cfg(test)]
    pub fn set_package_catalog_byte_limit(&mut self, max_bytes: usize) {
        self.limits.max_package_catalog_bytes = max_bytes;
    }

    #[cfg(test)]
    pub fn set_installation_entry_limit(&mut self, max_entries: usize) {
        self.limits.max_live_installations = max_entries;
    }

    #[cfg(test)]
    pub fn set_installation_directory_entry_limit(&mut self, max_entries: usize) {
        self.limits.max_installation_directory_entries = max_entries;
    }

    #[cfg(test)]
    pub fn set_receipt_byte_limit(&mut self, max_bytes: usize) {
        self.limits.max_receipt_bytes = max_bytes;
    }

    #[cfg(test)]
    pub fn set_receipt_catalog_byte_limit(&mut self, max_bytes: usize) {
        self.limits.max_receipt_catalog_bytes = max_bytes;
    }

    pub fn scan_receipts(&self) -> Result<ManagedReceiptIndex, ManagedStoreFatalError> {
        let Some(roots) = self.resolve_roots()? else {
            return Ok(ManagedReceiptIndex::default());
        };
        let Some(ref installations_root) = roots.installations_root else {
            return Ok(ManagedReceiptIndex::default());
        };
        let retired_ids = self.scan_retired_installation_ids(&roots)?;

        let entries = fs::read_dir(&installations_root.canonical_path).map_err(|error| {
            ManagedStoreFatalError::new(format!(
                "cannot read managed Skill installations directory: {error}"
            ))
        })?;
        let mut collected = Vec::new();
        let mut issues = Vec::new();
        let mut observed_entries = 0usize;
        for entry in entries {
            observed_entries = observed_entries.saturating_add(1);
            if observed_entries > self.limits.max_installation_directory_entries {
                return Ok(ManagedReceiptIndex {
                    receipts: Vec::new(),
                    issues: vec![ManagedStoreIssue::error(
                        SkillDiagnosticCode::TooManyEntries,
                        INSTALLATIONS_DIRECTORY,
                        format!(
                            "Managed Skill installations directory contains more than {} entries.",
                            self.limits.max_installation_directory_entries
                        ),
                    )],
                    truncated: true,
                });
            }
            match entry {
                Ok(entry) => collected.push(entry),
                Err(error) => issues.push(ManagedStoreIssue::error(
                    SkillDiagnosticCode::UnreadableEntry,
                    INSTALLATIONS_DIRECTORY,
                    format!("Cannot read a managed Skill installation entry: {error}"),
                )),
            }
        }
        collected.sort_by_key(fs::DirEntry::file_name);

        let live_entries = collected
            .iter()
            .filter(|entry| !is_hidden_managed_entry(&entry.file_name()))
            .count();
        if live_entries > self.limits.max_live_installations {
            return Ok(ManagedReceiptIndex {
                receipts: Vec::new(),
                issues: vec![ManagedStoreIssue::error(
                    SkillDiagnosticCode::TooManyEntries,
                    INSTALLATIONS_DIRECTORY,
                    format!(
                        "Managed Skill installations contain more than {} live entries.",
                        self.limits.max_live_installations
                    ),
                )],
                truncated: true,
            });
        }

        let mut receipt_budget = ByteBudget::new(self.limits.max_receipt_catalog_bytes);
        let mut receipts = Vec::new();
        let mut truncated = false;
        for entry in collected {
            let file_name = entry.file_name();
            if is_hidden_managed_entry(&file_name) {
                // Atomic writers may temporarily stage hidden entries. They
                // remain subject to the global entry budget but are never
                // interpreted as installation receipts.
                continue;
            }
            let location = format!(
                "{INSTALLATIONS_DIRECTORY}/{}",
                display_os_component(&file_name)
            );
            let Some(file_name_text) = file_name.to_str() else {
                issues.push(ManagedStoreIssue::error(
                    SkillDiagnosticCode::UnsupportedPathEncoding,
                    location,
                    "Managed Skill receipt names must be valid UTF-8.",
                ));
                continue;
            };
            let Some(id_text) = file_name_text.strip_suffix(".json") else {
                issues.push(ManagedStoreIssue::error(
                    SkillDiagnosticCode::InvalidInstallationReceipt,
                    location,
                    "Managed Skill installation entries must use `<canonical-uuid>.json` names.",
                ));
                continue;
            };
            let installation_id = match SkillInstallationId::parse(id_text) {
                Ok(installation_id) => installation_id,
                Err(error) => {
                    issues.push(ManagedStoreIssue::error(
                        SkillDiagnosticCode::InvalidInstallationReceipt,
                        location,
                        format!("Invalid managed Skill receipt name: {error}"),
                    ));
                    continue;
                }
            };
            if retired_ids.contains(&installation_id) {
                issues.push(ManagedStoreIssue::error(
                    SkillDiagnosticCode::InvalidInstallationReceipt,
                    location,
                    "A live receipt was ignored because this installation identity is durably retired.",
                ));
                continue;
            }

            match self.read_receipt_path(
                &roots,
                installations_root,
                &entry.path(),
                &location,
                &installation_id,
                &mut receipt_budget,
            ) {
                Ok(receipt) => receipts.push(receipt),
                Err(ManagedStoreLoadError::NotFound) => issues.push(ManagedStoreIssue::error(
                    SkillDiagnosticCode::PathChangedDuringRead,
                    location,
                    "Managed Skill receipt disappeared while it was inspected.",
                )),
                Err(ManagedStoreLoadError::Invalid(issue))
                | Err(ManagedStoreLoadError::Unavailable(issue)) => {
                    if issue.code == SkillDiagnosticCode::CatalogTooLarge {
                        truncated = true;
                        issues.push(issue);
                        break;
                    }
                    issues.push(issue);
                }
            }
        }

        verify_checked_directory(
            installations_root,
            "The installations directory changed while receipts were read.",
        )
        .map_err(ManagedStoreFatalError::new)?;
        verify_checked_directory(
            &roots.store_root,
            "The managed Skill store changed while receipts were read.",
        )
        .map_err(ManagedStoreFatalError::new)?;

        Ok(ManagedReceiptIndex {
            receipts,
            issues,
            truncated,
        })
    }

    pub fn load_receipt(
        &self,
        installation_id: &SkillInstallationId,
    ) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
        let roots = self
            .resolve_roots()
            .map_err(|error| unavailable_issue(INSTALLATIONS_DIRECTORY, error.to_string()))?
            .ok_or(ManagedStoreLoadError::NotFound)?;
        let installations_root = roots
            .installations_root
            .as_ref()
            .ok_or(ManagedStoreLoadError::NotFound)?;
        let file_name = format!("{installation_id}.{RECEIPT_EXTENSION}");
        if self.retired_identity_exists(&roots, installation_id, &file_name)? {
            return Err(ManagedStoreLoadError::NotFound);
        }
        let location = format!("{INSTALLATIONS_DIRECTORY}/{file_name}");
        let path = exact_child_path(
            installations_root,
            OsStr::new(&file_name),
            self.limits.max_installation_directory_entries,
        )
        .map_err(|error| map_directory_error(&location, error))?
        .ok_or(ManagedStoreLoadError::NotFound)?;
        let mut byte_budget = ByteBudget::new(self.limits.max_receipt_bytes.saturating_add(1));
        self.read_receipt_path(
            &roots,
            installations_root,
            &path,
            &location,
            installation_id,
            &mut byte_budget,
        )
    }

    pub fn load_complete_package(
        &self,
        package: &InstalledPackageRef,
    ) -> Result<ManagedCompletePackageSnapshot, ManagedStoreLoadError> {
        let mut byte_budget = ByteBudget::new(
            MAX_SKILL_PACKAGE_BYTES
                .saturating_add(MAX_SKILL_PACKAGE_MANIFEST_BYTES)
                .saturating_add(1),
        );
        self.load_package_internal(package, &mut byte_budget, true)
    }

    pub fn load_package_with_budget(
        &self,
        package: &InstalledPackageRef,
        byte_budget: &mut ByteBudget,
    ) -> Result<ManagedPackageSnapshot, ManagedStoreLoadError> {
        self.load_package_internal(package, byte_budget, false)
            .map(|snapshot| snapshot.package)
    }

    fn load_package_internal(
        &self,
        package: &InstalledPackageRef,
        byte_budget: &mut ByteBudget,
        include_resource_bytes: bool,
    ) -> Result<ManagedCompletePackageSnapshot, ManagedStoreLoadError> {
        let roots = self
            .resolve_roots()
            .map_err(|error| unavailable_issue(&package.relative_path, error.to_string()))?
            .ok_or(ManagedStoreLoadError::NotFound)?;
        let packages_root = checked_child_directory(&roots.store_root, PACKAGES_DIRECTORY)
            .map_err(|error| map_directory_error(&package.relative_path, error))?
            .ok_or(ManagedStoreLoadError::NotFound)?;
        let version_root = checked_child_directory(&packages_root, package.version_directory())
            .map_err(|error| map_directory_error(&package.relative_path, error))?
            .ok_or(ManagedStoreLoadError::NotFound)?;
        let package_root = checked_child_directory(&version_root, &package.digest_hex)
            .map_err(|error| map_directory_error(&package.relative_path, error))?
            .ok_or(ManagedStoreLoadError::NotFound)?;

        let snapshot = match package.format_version {
            SKILL_PACKAGE_FORMAT_VERSION => self.load_v1_package(
                package,
                &roots,
                &packages_root,
                &version_root,
                &package_root,
                byte_budget,
            ),
            SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3 => self
                .load_manifest_package(
                    package,
                    &roots,
                    &packages_root,
                    &version_root,
                    &package_root,
                    byte_budget,
                    include_resource_bytes,
                ),
            _ => Err(invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                &package.relative_path,
                format!(
                    "Unsupported package format version {}.",
                    package.format_version
                ),
            )),
        }?;

        verify_checked_directory(
            &package_root,
            "The managed package changed while it was read.",
        )
        .map_err(|reason| unavailable_issue(&package.relative_path, reason))?;
        verify_checked_directory(
            &version_root,
            "The managed package version directory changed while a package was read.",
        )
        .map_err(|reason| unavailable_issue(&package.relative_path, reason))?;
        verify_checked_directory(
            &packages_root,
            "The managed packages directory changed while a package was read.",
        )
        .map_err(|reason| unavailable_issue(&package.relative_path, reason))?;
        verify_checked_directory(
            &roots.store_root,
            "The managed Skill store changed while a package was read.",
        )
        .map_err(|reason| unavailable_issue(&package.relative_path, reason))?;
        Ok(snapshot)
    }

    #[allow(clippy::too_many_arguments)]
    fn load_v1_package(
        &self,
        package: &InstalledPackageRef,
        roots: &StoreRoots,
        _packages_root: &CheckedDirectory,
        _version_root: &CheckedDirectory,
        package_root: &CheckedDirectory,
        byte_budget: &mut ByteBudget,
    ) -> Result<ManagedCompletePackageSnapshot, ManagedStoreLoadError> {
        let entries = fs::read_dir(&package_root.canonical_path).map_err(|error| {
            unavailable_issue(
                &package.relative_path,
                format!("Cannot inspect managed Skill package: {error}"),
            )
        })?;
        let mut skill_path = None;
        let mut entry_count = 0usize;
        for entry in entries {
            entry_count = entry_count.saturating_add(1);
            if entry_count > 2 {
                return Err(invalid_issue(
                    SkillDiagnosticCode::UnexpectedPackageEntry,
                    &package.relative_path,
                    "Package format v1 permits only an exact-case SKILL.md file.",
                ));
            }
            let entry = entry.map_err(|error| {
                unavailable_issue(
                    &package.relative_path,
                    format!("Cannot inspect a managed Skill package entry: {error}"),
                )
            })?;
            if entry.file_name() == OsStr::new(SKILL_FILE_NAME) {
                if skill_path.replace(entry.path()).is_some() {
                    return Err(invalid_issue(
                        SkillDiagnosticCode::UnexpectedPackageEntry,
                        &package.relative_path,
                        "Package format v1 contains more than one SKILL.md entry.",
                    ));
                }
            } else {
                return Err(invalid_issue(
                    SkillDiagnosticCode::UnexpectedPackageEntry,
                    &package.relative_path,
                    "Package format v1 permits no sibling resources or additional entries.",
                ));
            }
        }
        let skill_path = skill_path.ok_or(ManagedStoreLoadError::NotFound)?;
        let bytes = read_checked_file(
            &skill_path,
            package_root,
            &roots.store_root,
            byte_budget,
            &package.relative_path,
            ManagedFileReadPolicy {
                max_bytes: MAX_SKILL_FILE_BYTES,
                too_large_code: SkillDiagnosticCode::SkillFileTooLarge,
            },
        )?;
        let actual_revision = package_revision(&bytes);
        if actual_revision != package.revision {
            return Err(invalid_issue(
                SkillDiagnosticCode::PackageRevisionMismatch,
                &package.relative_path,
                format!(
                    "Managed Skill package bytes do not match receipt revision `{}`.",
                    package.revision
                ),
            ));
        }

        Ok(ManagedCompletePackageSnapshot {
            package: ManagedPackageSnapshot {
                bytes,
                relative_path: package.relative_path.clone(),
                format_version: SKILL_PACKAGE_FORMAT_VERSION,
                resources: SkillResourceIndex::default(),
            },
            resource_bytes: Vec::new(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn load_manifest_package(
        &self,
        package: &InstalledPackageRef,
        roots: &StoreRoots,
        _packages_root: &CheckedDirectory,
        _version_root: &CheckedDirectory,
        package_root: &CheckedDirectory,
        byte_budget: &mut ByteBudget,
        include_resource_bytes: bool,
    ) -> Result<ManagedCompletePackageSnapshot, ManagedStoreLoadError> {
        let manifest_path = exact_child_path(
            package_root,
            OsStr::new(PACKAGE_MANIFEST_FILE),
            MAX_MANAGED_DIRECTORY_ENTRIES,
        )
        .map_err(|error| map_directory_error(&package.relative_path, error))?
        .ok_or(ManagedStoreLoadError::NotFound)?;
        let manifest_bytes = read_checked_file(
            &manifest_path,
            package_root,
            &roots.store_root,
            byte_budget,
            &package.relative_path,
            ManagedFileReadPolicy {
                max_bytes: MAX_SKILL_PACKAGE_MANIFEST_BYTES,
                too_large_code: SkillDiagnosticCode::InvalidPackageManifest,
            },
        )?;
        let manifest = PackageManifest::decode(&manifest_bytes)
            .map_err(|error| package_validation_issue(&package.relative_path, error))?;
        if manifest.format_version() != package.format_version {
            return Err(invalid_issue(
                SkillDiagnosticCode::InvalidPackageManifest,
                &package.relative_path,
                format!(
                    "Managed Skill package manifest declares format {}, but its receipt declares format {}.",
                    manifest.format_version(),
                    package.format_version
                ),
            ));
        }
        let actual_revision = manifest.revision();
        if actual_revision != package.revision {
            return Err(invalid_issue(
                SkillDiagnosticCode::PackageRevisionMismatch,
                &package.relative_path,
                format!(
                    "Managed Skill package manifest does not match receipt revision `{}`.",
                    package.revision
                ),
            ));
        }

        let skill_entry = manifest.entrypoint_entry();
        let skill_bytes = read_manifest_file(
            package_root,
            &roots.store_root,
            skill_entry.path(),
            usize::try_from(skill_entry.byte_length()).unwrap_or(usize::MAX),
            SkillDiagnosticCode::SkillFileTooLarge,
            byte_budget,
            &package.relative_path,
        )?;
        verify_manifest_file_bytes(skill_entry, &skill_bytes, &package.relative_path)?;

        let mut resource_bytes = Vec::new();
        if include_resource_bytes {
            resource_bytes.reserve(manifest.files().len().saturating_sub(1));
            for entry in manifest
                .files()
                .iter()
                .filter(|entry| entry.path() != SKILL_FILE_NAME)
            {
                let bytes = read_manifest_file(
                    package_root,
                    &roots.store_root,
                    entry.path(),
                    MAX_SKILL_RESOURCE_FILE_BYTES,
                    SkillDiagnosticCode::ResourceFileTooLarge,
                    byte_budget,
                    &package.relative_path,
                )?;
                verify_manifest_file_bytes(entry, &bytes, &package.relative_path)?;
                resource_bytes.push((entry.path().to_string(), bytes));
            }
        }

        Ok(ManagedCompletePackageSnapshot {
            package: ManagedPackageSnapshot {
                bytes: skill_bytes,
                relative_path: package.relative_path.clone(),
                format_version: package.format_version,
                resources: SkillResourceIndex::new(manifest.resource_descriptors()),
            },
            resource_bytes,
        })
    }

    fn resolve_roots(&self) -> Result<Option<StoreRoots>, ManagedStoreFatalError> {
        let Some(store_root) = checked_root_directory(&self.root)? else {
            return Ok(None);
        };
        let installations_root = checked_child_directory(&store_root, INSTALLATIONS_DIRECTORY)
            .map_err(|error| ManagedStoreFatalError::new(error.to_string()))?;
        let retired_installations_root =
            checked_child_directory(&store_root, RETIRED_INSTALLATIONS_DIRECTORY)
                .map_err(|error| ManagedStoreFatalError::new(error.to_string()))?;
        Ok(Some(StoreRoots {
            store_root,
            installations_root,
            retired_installations_root,
        }))
    }

    fn scan_retired_installation_ids(
        &self,
        roots: &StoreRoots,
    ) -> Result<BTreeSet<SkillInstallationId>, ManagedStoreFatalError> {
        let Some(retired_root) = roots.retired_installations_root.as_ref() else {
            return Ok(BTreeSet::new());
        };
        let mut ids = BTreeSet::new();
        for (index, entry) in fs::read_dir(&retired_root.canonical_path)
            .map_err(|error| {
                ManagedStoreFatalError::new(format!(
                    "cannot read retired Skill installation identities: {error}"
                ))
            })?
            .enumerate()
        {
            if index >= MAX_RETIRED_INSTALLATION_ENTRIES {
                return Err(ManagedStoreFatalError::new(format!(
                    "retired Skill installation identities exceed the {MAX_RETIRED_INSTALLATION_ENTRIES}-entry limit"
                )));
            }
            let entry = entry.map_err(|error| {
                ManagedStoreFatalError::new(format!(
                    "cannot inspect a retired Skill installation identity: {error}"
                ))
            })?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                ManagedStoreFatalError::new(format!(
                    "cannot inspect a retired Skill installation identity: {error}"
                ))
            })?;
            if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
                return Err(ManagedStoreFatalError::new(
                    "retired Skill installation identities must be plain files",
                ));
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                ManagedStoreFatalError::new(
                    "retired Skill installation identity names must be valid UTF-8",
                )
            })?;
            let id = name
                .strip_suffix(".json")
                .ok_or_else(|| {
                    ManagedStoreFatalError::new(
                        "retired Skill installation identities must use `<canonical-uuid>.json` names",
                    )
                })
                .and_then(|value| {
                    SkillInstallationId::parse(value)
                        .map_err(|error| ManagedStoreFatalError::new(error.to_string()))
                })?;
            if !ids.insert(id) {
                return Err(ManagedStoreFatalError::new(
                    "retired Skill installation identity names are not unique",
                ));
            }
        }
        verify_checked_directory(
            retired_root,
            "The retired Skill identity ledger changed while it was read.",
        )
        .map_err(ManagedStoreFatalError::new)?;
        Ok(ids)
    }

    fn retired_identity_exists(
        &self,
        roots: &StoreRoots,
        installation_id: &SkillInstallationId,
        file_name: &str,
    ) -> Result<bool, ManagedStoreLoadError> {
        let Some(retired_root) = roots.retired_installations_root.as_ref() else {
            return Ok(false);
        };
        let location = format!("{RETIRED_INSTALLATIONS_DIRECTORY}/{file_name}");
        let Some(path) = exact_child_path(
            retired_root,
            OsStr::new(file_name),
            MAX_RETIRED_INSTALLATION_ENTRIES,
        )
        .map_err(|error| map_directory_error(&location, error))?
        else {
            return Ok(false);
        };
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            unavailable_issue(
                &location,
                format!("Cannot inspect retired Skill identity `{installation_id}`: {error}"),
            )
        })?;
        if is_symlink_or_reparse(&metadata) || !metadata.is_file() {
            return Err(invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                &location,
                "Retired Skill installation identities must be plain files.",
            ));
        }
        verify_checked_directory(
            retired_root,
            "The retired Skill identity ledger changed while it was inspected.",
        )
        .map_err(|reason| unavailable_issue(&location, reason))?;
        Ok(true)
    }

    fn read_receipt_path(
        &self,
        roots: &StoreRoots,
        installations_root: &CheckedDirectory,
        path: &Path,
        location: &str,
        expected_id: &SkillInstallationId,
        byte_budget: &mut ByteBudget,
    ) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
        let bytes = read_checked_file(
            path,
            installations_root,
            &roots.store_root,
            byte_budget,
            location,
            ManagedFileReadPolicy {
                max_bytes: self.limits.max_receipt_bytes,
                too_large_code: SkillDiagnosticCode::InvalidInstallationReceipt,
            },
        )?;
        let receipt = decode_receipt(&bytes, location)?;
        if &receipt.installation_id != expected_id {
            return Err(invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                location,
                format!(
                    "Receipt installation id `{}` does not match its canonical file name `{expected_id}`.",
                    receipt.installation_id
                ),
            ));
        }

        verify_checked_directory(
            installations_root,
            "The installations directory changed while a receipt was read.",
        )
        .map_err(|reason| unavailable_issue(location, reason))?;
        verify_checked_directory(
            &roots.store_root,
            "The managed Skill store changed while a receipt was read.",
        )
        .map_err(|reason| unavailable_issue(location, reason))?;
        Ok(receipt)
    }
}

#[derive(Debug, Default)]
pub(super) struct ManagedReceiptIndex {
    pub receipts: Vec<InstalledSkillReceipt>,
    pub issues: Vec<ManagedStoreIssue>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub(super) struct InstalledSkillReceipt {
    pub installation_id: SkillInstallationId,
    pub receipt_schema_version: u32,
    pub generation: u64,
    pub package: InstalledPackageRef,
    pub provenance: SkillInstallationProvenance,
    /// Transitional compatibility projection for installer call sites that
    /// have not yet moved from audit origin to typed provenance.
    pub _origin: SkillPackageOrigin,
    pub installation_revision: SkillInstallationRevision,
    pub installed_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl InstalledSkillReceipt {
    pub(super) fn new_v2(
        installation_id: SkillInstallationId,
        generation: u64,
        package_format_version: u32,
        package_revision: SkillRevision,
        provenance: SkillInstallationProvenance,
        installed_at_unix_ms: u64,
        updated_at_unix_ms: u64,
    ) -> Result<Self, String> {
        if generation == 0 {
            return Err("managed Skill receipt generation must be greater than zero".to_string());
        }
        if updated_at_unix_ms < installed_at_unix_ms {
            return Err(
                "managed Skill receipt updated time cannot precede its installed time".to_string(),
            );
        }
        let package = InstalledPackageRef::from_format_and_revision(
            package_format_version,
            package_revision,
        )?;
        let installation_revision = installation_revision(
            &installation_id,
            generation,
            package.format_version,
            &package.revision,
            &provenance,
        );
        let origin = SkillPackageOrigin::new(
            provenance.authority().provider(),
            provenance.authority().payload(),
        )
        .expect("validated provenance authority satisfies the legacy origin envelope");
        Ok(Self {
            installation_id,
            receipt_schema_version: RECEIPT_SCHEMA_VERSION_V2,
            generation,
            package,
            provenance,
            _origin: origin,
            installation_revision,
            installed_at_unix_ms,
            updated_at_unix_ms,
        })
    }

    pub(super) fn is_legacy_v1(&self) -> bool {
        self.receipt_schema_version == RECEIPT_SCHEMA_VERSION_V1
    }
}

#[derive(Debug, Clone)]
pub(super) struct InstalledPackageRef {
    pub format_version: u32,
    pub revision: SkillRevision,
    pub digest_hex: String,
    pub relative_path: String,
}

impl InstalledPackageRef {
    pub fn from_format_and_revision(
        format_version: u32,
        revision: SkillRevision,
    ) -> Result<Self, String> {
        let digest_hex = package_digest_hex(format_version, &revision)?;
        let relative_path = managed_package_relative_path_from_digest(format_version, &digest_hex)?;
        Ok(Self {
            format_version,
            revision,
            digest_hex,
            relative_path,
        })
    }

    pub fn version_directory(&self) -> &'static str {
        match self.format_version {
            SKILL_PACKAGE_FORMAT_VERSION => PACKAGE_V1_DIRECTORY,
            SKILL_PACKAGE_FORMAT_VERSION_V2 => PACKAGE_V2_DIRECTORY,
            SKILL_PACKAGE_FORMAT_VERSION_V3 => PACKAGE_V3_DIRECTORY,
            _ => unreachable!("InstalledPackageRef validates package format"),
        }
    }
}

#[derive(Debug)]
pub(super) struct ManagedPackageSnapshot {
    pub bytes: Vec<u8>,
    pub relative_path: String,
    pub format_version: u32,
    pub resources: SkillResourceIndex,
}

#[derive(Debug)]
pub(super) struct ManagedCompletePackageSnapshot {
    pub package: ManagedPackageSnapshot,
    pub resource_bytes: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub(super) struct ManagedStoreIssue {
    pub code: SkillDiagnosticCode,
    pub severity: SkillDiagnosticSeverity,
    pub location: String,
    pub message: String,
}

impl ManagedStoreIssue {
    pub(super) fn error(
        code: SkillDiagnosticCode,
        location: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: SkillDiagnosticSeverity::Error,
            location: location.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug)]
pub(super) enum ManagedStoreLoadError {
    NotFound,
    Invalid(ManagedStoreIssue),
    Unavailable(ManagedStoreIssue),
}

#[derive(Debug)]
pub(super) struct ManagedStoreFatalError {
    reason: String,
}

impl ManagedStoreFatalError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for ManagedStoreFatalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(formatter)
    }
}

#[derive(Debug)]
struct StoreRoots {
    store_root: CheckedDirectory,
    installations_root: Option<CheckedDirectory>,
    retired_installations_root: Option<CheckedDirectory>,
}

#[derive(Debug, Clone)]
struct CheckedDirectory {
    lexical_path: PathBuf,
    canonical_path: PathBuf,
}

#[derive(Debug)]
enum DirectoryCheckError {
    InvalidComponent(String),
    Symlink(String),
    NotDirectory(String),
    EscapesParent(String),
    Unavailable(String),
}

impl std::fmt::Display for DirectoryCheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidComponent(reason)
            | Self::Symlink(reason)
            | Self::NotDirectory(reason)
            | Self::EscapesParent(reason)
            | Self::Unavailable(reason) => reason.fmt(formatter),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptSchemaProbe {
    schema_version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptDocumentV1 {
    schema_version: u32,
    installation_id: String,
    package: ReceiptPackageDocument,
    origin: ReceiptOriginDocument,
    installed_at_unix_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptDocumentV2 {
    schema_version: u32,
    installation_id: String,
    generation: u64,
    package: ReceiptPackageDocument,
    provenance: ReceiptProvenanceDocument,
    installation_revision: String,
    installed_at_unix_ms: u64,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptPackageDocument {
    format_version: u32,
    revision: String,
    entrypoint: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptOriginDocument {
    provider: String,
    reference: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptProvenanceDocument {
    authority: ReceiptProvenanceRecordDocument,
    refresh: Option<ReceiptProvenanceRecordDocument>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptProvenanceRecordDocument {
    provider: String,
    schema_version: u32,
    payload: String,
}

fn decode_receipt(
    bytes: &[u8],
    location: &str,
) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
    if bytes.contains(&0) {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            "Managed Skill receipt contains a NUL byte.",
        ));
    }
    let probe: ReceiptSchemaProbe = serde_json::from_slice(bytes).map_err(|error| {
        invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Managed Skill receipt is not valid strict JSON: {error}"),
        )
    })?;
    match probe.schema_version {
        RECEIPT_SCHEMA_VERSION_V1 => {
            let document: ReceiptDocumentV1 = serde_json::from_slice(bytes).map_err(|error| {
                invalid_issue(
                    SkillDiagnosticCode::InvalidInstallationReceipt,
                    location,
                    format!("Managed Skill receipt v1 is not valid strict JSON: {error}"),
                )
            })?;
            validate_receipt_v1(document, location)
        }
        RECEIPT_SCHEMA_VERSION_V2 => {
            let document: ReceiptDocumentV2 = serde_json::from_slice(bytes).map_err(|error| {
                invalid_issue(
                    SkillDiagnosticCode::InvalidInstallationReceipt,
                    location,
                    format!("Managed Skill receipt v2 is not valid strict JSON: {error}"),
                )
            })?;
            validate_receipt_v2(document, location)
        }
        unsupported => Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Unsupported managed Skill receipt schema version {unsupported}."),
        )),
    }
}

fn validate_receipt_v1(
    document: ReceiptDocumentV1,
    location: &str,
) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
    if document.schema_version != RECEIPT_SCHEMA_VERSION_V1 {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            "Managed Skill receipt v1 contains an inconsistent schema version.",
        ));
    }
    let installation_id = validate_installation_id(document.installation_id, location)?;
    let package = validate_receipt_package(document.package, location)?;
    let origin = SkillPackageOrigin::new(document.origin.provider, document.origin.reference)
        .map_err(|error| {
            invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                location,
                format!("Invalid receipt origin: {error}"),
            )
        })?;
    // Legacy audit metadata never gains refresh authority during normalization.
    let provenance = SkillInstallationProvenance::from_legacy_origin(&origin);
    let generation = 1;
    let installation_revision = installation_revision(
        &installation_id,
        generation,
        package.format_version,
        &package.revision,
        &provenance,
    );
    Ok(InstalledSkillReceipt {
        installation_id,
        receipt_schema_version: RECEIPT_SCHEMA_VERSION_V1,
        generation,
        package,
        provenance,
        _origin: origin,
        installation_revision,
        installed_at_unix_ms: document.installed_at_unix_ms,
        updated_at_unix_ms: document.installed_at_unix_ms,
    })
}

fn validate_receipt_v2(
    document: ReceiptDocumentV2,
    location: &str,
) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
    if document.schema_version != RECEIPT_SCHEMA_VERSION_V2 {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            "Managed Skill receipt v2 contains an inconsistent schema version.",
        ));
    }
    let installation_id = validate_installation_id(document.installation_id, location)?;
    let package = validate_receipt_package(document.package, location)?;
    let provenance = validate_receipt_provenance(document.provenance, location)?;
    let persisted_revision = SkillInstallationRevision::parse(document.installation_revision)
        .map_err(|error| {
            invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                location,
                format!("Invalid installation revision: {error}"),
            )
        })?;
    let receipt = InstalledSkillReceipt::new_v2(
        installation_id,
        document.generation,
        package.format_version,
        package.revision,
        provenance,
        document.installed_at_unix_ms,
        document.updated_at_unix_ms,
    )
    .map_err(|reason| {
        invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            reason,
        )
    })?;
    if receipt.installation_revision != persisted_revision {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            "Managed Skill receipt installation revision does not match its normalized lifecycle state.",
        ));
    }
    Ok(receipt)
}

fn validate_installation_id(
    value: String,
    location: &str,
) -> Result<SkillInstallationId, ManagedStoreLoadError> {
    SkillInstallationId::parse(value).map_err(|error| {
        invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Invalid receipt installation id: {error}"),
        )
    })
}

fn validate_receipt_package(
    document: ReceiptPackageDocument,
    location: &str,
) -> Result<InstalledPackageRef, ManagedStoreLoadError> {
    if !matches!(
        document.format_version,
        SKILL_PACKAGE_FORMAT_VERSION
            | SKILL_PACKAGE_FORMAT_VERSION_V2
            | SKILL_PACKAGE_FORMAT_VERSION_V3
    ) {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!(
                "Unsupported managed Skill package format version {}.",
                document.format_version
            ),
        ));
    }
    if document.entrypoint != SKILL_FILE_NAME {
        return Err(invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Managed Skill package entrypoint must be exactly `{SKILL_FILE_NAME}`."),
        ));
    }
    let revision = SkillRevision::parse(document.revision).map_err(|error| {
        invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Invalid package revision: {error}"),
        )
    })?;
    InstalledPackageRef::from_format_and_revision(document.format_version, revision).map_err(
        |reason| {
            invalid_issue(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                location,
                reason,
            )
        },
    )
}

fn validate_receipt_provenance(
    document: ReceiptProvenanceDocument,
    location: &str,
) -> Result<SkillInstallationProvenance, ManagedStoreLoadError> {
    let authority = SkillInstallationAuthority::new(
        document.authority.provider,
        document.authority.schema_version,
        document.authority.payload,
    )
    .map_err(|error| {
        invalid_issue(
            SkillDiagnosticCode::InvalidInstallationReceipt,
            location,
            format!("Invalid receipt acquisition authority: {error}"),
        )
    })?;
    let refresh = document
        .refresh
        .map(|refresh| {
            SkillInstallationRefresh::new(refresh.provider, refresh.schema_version, refresh.payload)
                .map_err(|error| {
                    invalid_issue(
                        SkillDiagnosticCode::InvalidInstallationReceipt,
                        location,
                        format!("Invalid receipt refresh provenance: {error}"),
                    )
                })
        })
        .transpose()?;
    Ok(SkillInstallationProvenance::new(authority, refresh))
}

pub(super) fn encode_receipt_v2(receipt: &InstalledSkillReceipt) -> Result<Vec<u8>, String> {
    if receipt.generation == 0 {
        return Err("managed Skill receipt generation must be greater than zero".to_string());
    }
    if receipt.updated_at_unix_ms < receipt.installed_at_unix_ms {
        return Err(
            "managed Skill receipt updated time cannot precede its installed time".to_string(),
        );
    }
    let expected_revision = installation_revision(
        &receipt.installation_id,
        receipt.generation,
        receipt.package.format_version,
        &receipt.package.revision,
        &receipt.provenance,
    );
    if receipt.installation_revision != expected_revision {
        return Err(
            "managed Skill receipt installation revision does not match its lifecycle state"
                .to_string(),
        );
    }
    let document = ReceiptDocumentV2 {
        schema_version: RECEIPT_SCHEMA_VERSION_V2,
        installation_id: receipt.installation_id.as_str().to_string(),
        generation: receipt.generation,
        package: ReceiptPackageDocument {
            format_version: receipt.package.format_version,
            revision: receipt.package.revision.as_str().to_string(),
            entrypoint: SKILL_FILE_NAME.to_string(),
        },
        provenance: ReceiptProvenanceDocument {
            authority: provenance_record_document(
                receipt.provenance.authority().provider(),
                receipt.provenance.authority().schema_version(),
                receipt.provenance.authority().payload(),
            ),
            refresh: receipt.provenance.refresh().map(|refresh| {
                provenance_record_document(
                    refresh.provider(),
                    refresh.schema_version(),
                    refresh.payload(),
                )
            }),
        },
        installation_revision: receipt.installation_revision.as_str().to_string(),
        installed_at_unix_ms: receipt.installed_at_unix_ms,
        updated_at_unix_ms: receipt.updated_at_unix_ms,
    };
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| format!("cannot serialize managed Skill receipt: {error}"))?;
    if bytes.len() > MAX_RECEIPT_BYTES {
        return Err(format!(
            "serialized managed Skill receipt exceeds {MAX_RECEIPT_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

fn provenance_record_document(
    provider: &str,
    schema_version: u32,
    payload: &str,
) -> ReceiptProvenanceRecordDocument {
    ReceiptProvenanceRecordDocument {
        provider: provider.to_string(),
        schema_version,
        payload: payload.to_string(),
    }
}

/// Test-only fixture encoder for callers that model the legacy origin API.
#[cfg(test)]
pub(super) fn encode_receipt(
    installation_id: &SkillInstallationId,
    package_format_version: u32,
    package_revision: &SkillRevision,
    origin: &SkillPackageOrigin,
    installed_at_unix_ms: u64,
) -> Result<Vec<u8>, String> {
    let receipt = InstalledSkillReceipt::new_v2(
        installation_id.clone(),
        1,
        package_format_version,
        package_revision.clone(),
        SkillInstallationProvenance::from_legacy_origin(origin),
        installed_at_unix_ms,
        installed_at_unix_ms,
    )?;
    encode_receipt_v2(&receipt)
}

pub(super) fn managed_package_relative_path(revision: &SkillRevision) -> Result<String, String> {
    let format_version = revision_format_version(revision)?;
    let digest = package_digest_hex(format_version, revision)?;
    managed_package_relative_path_from_digest(format_version, &digest)
}

fn managed_package_relative_path_from_digest(
    format_version: u32,
    digest_hex: &str,
) -> Result<String, String> {
    let version = package_version_directory(format_version)?;
    Ok(format!(
        "{PACKAGES_DIRECTORY}/{version}/{digest_hex}/{SKILL_FILE_NAME}"
    ))
}

fn package_digest_hex(format_version: u32, revision: &SkillRevision) -> Result<String, String> {
    let prefix = match format_version {
        SKILL_PACKAGE_FORMAT_VERSION => PACKAGE_REVISION_PREFIX,
        SKILL_PACKAGE_FORMAT_VERSION_V2 => PACKAGE_REVISION_V2_PREFIX,
        SKILL_PACKAGE_FORMAT_VERSION_V3 => PACKAGE_REVISION_V3_PREFIX,
        _ => {
            return Err(format!(
                "Unsupported package format version {format_version}."
            ))
        }
    };
    let Some(digest) = revision.as_str().strip_prefix(prefix) else {
        return Err(format!(
            "Package format {format_version} revision must start with `{prefix}`."
        ));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(
            "Package revision digest must contain exactly 64 lowercase hexadecimal characters."
                .to_string(),
        );
    }
    Ok(digest.to_string())
}

fn revision_format_version(revision: &SkillRevision) -> Result<u32, String> {
    if revision.as_str().starts_with(PACKAGE_REVISION_PREFIX) {
        Ok(SKILL_PACKAGE_FORMAT_VERSION)
    } else if revision.as_str().starts_with(PACKAGE_REVISION_V2_PREFIX) {
        Ok(SKILL_PACKAGE_FORMAT_VERSION_V2)
    } else if revision.as_str().starts_with(PACKAGE_REVISION_V3_PREFIX) {
        Ok(SKILL_PACKAGE_FORMAT_VERSION_V3)
    } else {
        Err("Managed package revision uses an unsupported format prefix.".to_string())
    }
}

fn package_version_directory(format_version: u32) -> Result<&'static str, String> {
    match format_version {
        SKILL_PACKAGE_FORMAT_VERSION => Ok(PACKAGE_V1_DIRECTORY),
        SKILL_PACKAGE_FORMAT_VERSION_V2 => Ok(PACKAGE_V2_DIRECTORY),
        SKILL_PACKAGE_FORMAT_VERSION_V3 => Ok(PACKAGE_V3_DIRECTORY),
        _ => Err(format!(
            "Unsupported package format version {format_version}."
        )),
    }
}

fn checked_root_directory(path: &Path) -> Result<Option<CheckedDirectory>, ManagedStoreFatalError> {
    let Some(metadata) = metadata_if_present(path).map_err(|error| {
        ManagedStoreFatalError::new(format!("cannot inspect managed Skill store: {error}"))
    })?
    else {
        return Ok(None);
    };
    if is_symlink_or_reparse(&metadata) {
        return Err(ManagedStoreFatalError::new(
            "managed Skill store root cannot be a symlink or reparse point",
        ));
    }
    if !metadata.is_dir() {
        return Err(ManagedStoreFatalError::new(
            "managed Skill store root is not a directory",
        ));
    }
    let canonical_path = path.canonicalize().map_err(|error| {
        ManagedStoreFatalError::new(format!("cannot resolve managed Skill store: {error}"))
    })?;
    let checked = CheckedDirectory {
        lexical_path: path.to_path_buf(),
        canonical_path,
    };
    verify_checked_directory(
        &checked,
        "The managed Skill store changed while it was inspected.",
    )
    .map_err(ManagedStoreFatalError::new)?;
    Ok(Some(checked))
}

fn checked_child_directory(
    parent: &CheckedDirectory,
    name: &str,
) -> Result<Option<CheckedDirectory>, DirectoryCheckError> {
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || name == "."
        || name == ".."
        || name.chars().any(char::is_control)
    {
        return Err(DirectoryCheckError::InvalidComponent(
            "managed Skill directory component is invalid".to_string(),
        ));
    }
    let Some(lexical_path) =
        exact_child_path(parent, OsStr::new(name), MAX_MANAGED_DIRECTORY_ENTRIES)?
    else {
        return Ok(None);
    };
    let metadata = match metadata_if_present(&lexical_path) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => {
            return Err(DirectoryCheckError::Unavailable(format!(
                "managed Skill directory `{name}` disappeared while it was inspected"
            )))
        }
        Err(error) => {
            return Err(DirectoryCheckError::Unavailable(format!(
                "cannot inspect managed Skill directory `{name}`: {error}"
            )))
        }
    };
    if is_symlink_or_reparse(&metadata) {
        return Err(DirectoryCheckError::Symlink(format!(
            "managed Skill directory `{name}` cannot be a symlink or reparse point"
        )));
    }
    if !metadata.is_dir() {
        return Err(DirectoryCheckError::NotDirectory(format!(
            "managed Skill path `{name}` is not a directory"
        )));
    }
    let canonical_path = lexical_path.canonicalize().map_err(|error| {
        DirectoryCheckError::Unavailable(format!(
            "cannot resolve managed Skill directory `{name}`: {error}"
        ))
    })?;
    if !canonical_path.starts_with(&parent.canonical_path) {
        return Err(DirectoryCheckError::EscapesParent(format!(
            "managed Skill directory `{name}` resolves outside its parent"
        )));
    }
    let checked = CheckedDirectory {
        lexical_path,
        canonical_path,
    };
    verify_checked_directory(
        &checked,
        "The managed Skill directory changed while it was inspected.",
    )
    .map_err(DirectoryCheckError::Unavailable)?;
    Ok(Some(checked))
}

/// Locates a child by the exact directory-entry spelling exposed by the
/// filesystem. `canonicalize()` cannot prove this property on case-insensitive
/// filesystems because it may preserve the caller's spelling.
fn exact_child_path(
    parent: &CheckedDirectory,
    expected_name: &OsStr,
    max_entries: usize,
) -> Result<Option<PathBuf>, DirectoryCheckError> {
    let entries = fs::read_dir(&parent.canonical_path).map_err(|error| {
        DirectoryCheckError::Unavailable(format!(
            "cannot inspect managed Skill directory entries: {error}"
        ))
    })?;
    let mut observed_entries = 0usize;
    let mut exact_path = None;
    for entry in entries {
        observed_entries = observed_entries.saturating_add(1);
        if observed_entries > max_entries {
            return Err(DirectoryCheckError::Unavailable(format!(
                "managed Skill directory contains more than {max_entries} entries"
            )));
        }
        let entry = entry.map_err(|error| {
            DirectoryCheckError::Unavailable(format!(
                "cannot inspect a managed Skill directory entry: {error}"
            ))
        })?;
        if entry.file_name() == expected_name {
            exact_path = Some(parent.lexical_path.join(expected_name));
        }
    }
    if exact_path.is_some() {
        return Ok(exact_path);
    }

    // On a case-insensitive filesystem the lexical lookup can still resolve a
    // differently-cased entry. Treat that as a format violation instead of
    // silently accepting a store that a case-sensitive reader would reject.
    let candidate = parent.lexical_path.join(expected_name);
    match metadata_if_present(&candidate) {
        Ok(Some(_)) => Err(DirectoryCheckError::InvalidComponent(format!(
            "managed Skill entry `{}` does not use its required exact-case name",
            display_os_component(expected_name)
        ))),
        Ok(None) => Ok(None),
        Err(error) => Err(DirectoryCheckError::Unavailable(format!(
            "cannot inspect managed Skill entry `{}`: {error}",
            display_os_component(expected_name)
        ))),
    }
}

fn is_hidden_managed_entry(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| name.starts_with('.'))
}

fn verify_checked_directory(directory: &CheckedDirectory, message: &str) -> Result<(), String> {
    verify_plain_directory(&directory.lexical_path, &directory.canonical_path, message)
        .map_err(|issue| issue.message)
}

#[derive(Debug, Clone, Copy)]
struct ManagedFileReadPolicy {
    max_bytes: usize,
    too_large_code: SkillDiagnosticCode,
}

fn read_checked_file(
    path: &Path,
    canonical_parent: &CheckedDirectory,
    store_root: &CheckedDirectory,
    byte_budget: &mut ByteBudget,
    location: &str,
    policy: ManagedFileReadPolicy,
) -> Result<Vec<u8>, ManagedStoreLoadError> {
    let metadata = match metadata_if_present(path) {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return Err(ManagedStoreLoadError::NotFound),
        Err(error) => {
            return Err(unavailable_issue(
                location,
                format!("Cannot inspect managed Skill file: {error}"),
            ))
        }
    };
    if is_symlink_or_reparse(&metadata) {
        return Err(invalid_issue(
            SkillDiagnosticCode::SymlinkNotAllowed,
            location,
            "Managed Skill access does not follow symlinks or reparse points.",
        ));
    }
    if !metadata.is_file() {
        return Err(invalid_issue(
            SkillDiagnosticCode::MissingSkillFile,
            location,
            "Managed Skill path is not a regular file.",
        ));
    }
    let canonical_path = path.canonicalize().map_err(|error| {
        unavailable_issue(
            location,
            format!("Cannot resolve managed Skill file: {error}"),
        )
    })?;
    if !canonical_path.starts_with(&canonical_parent.canonical_path)
        || !canonical_path.starts_with(&store_root.canonical_path)
    {
        return Err(invalid_issue(
            SkillDiagnosticCode::RootEscapesWorkspace,
            location,
            "Managed Skill file resolves outside the configured store.",
        ));
    }
    read_bounded_verified(
        path,
        &canonical_path,
        &metadata,
        &canonical_parent.canonical_path,
        &store_root.canonical_path,
        policy.max_bytes,
        byte_budget,
    )
    .map_err(|error| match error {
        BoundedReadError::TooLarge => invalid_issue(
            policy.too_large_code,
            location,
            format!("Managed Skill file exceeds {} bytes.", policy.max_bytes),
        ),
        BoundedReadError::CatalogBudgetExceeded => invalid_issue(
            SkillDiagnosticCode::CatalogTooLarge,
            location,
            "Managed Skill catalog exceeded its byte budget.",
        ),
        BoundedReadError::Io(error) => {
            unavailable_issue(location, format!("Cannot read managed Skill file: {error}"))
        }
        BoundedReadError::PathChanged(reason) => unavailable_issue(
            location,
            format!("Managed Skill file changed while it was read: {reason}"),
        ),
    })
}

fn read_manifest_file(
    package_root: &CheckedDirectory,
    store_root: &CheckedDirectory,
    relative_path: &str,
    max_bytes: usize,
    too_large_code: SkillDiagnosticCode,
    byte_budget: &mut ByteBudget,
    package_location: &str,
) -> Result<Vec<u8>, ManagedStoreLoadError> {
    let components = relative_path.split('/').collect::<Vec<_>>();
    let (file_name, directories) = components.split_last().ok_or_else(|| {
        invalid_issue(
            SkillDiagnosticCode::InvalidResourcePath,
            package_location,
            "Managed Skill package contains an empty resource path.",
        )
    })?;
    let mut parent = package_root.clone();
    for component in directories {
        parent = checked_child_directory(&parent, component)
            .map_err(|error| map_directory_error(package_location, error))?
            .ok_or(ManagedStoreLoadError::NotFound)?;
    }
    let path = exact_child_path(
        &parent,
        OsStr::new(file_name),
        MAX_MANAGED_DIRECTORY_ENTRIES,
    )
    .map_err(|error| map_directory_error(package_location, error))?
    .ok_or(ManagedStoreLoadError::NotFound)?;
    read_checked_file(
        &path,
        &parent,
        store_root,
        byte_budget,
        &format!("{package_location}:{relative_path}"),
        ManagedFileReadPolicy {
            max_bytes,
            too_large_code,
        },
    )
}

fn verify_manifest_file_bytes(
    entry: &PackageManifestEntry,
    bytes: &[u8],
    package_location: &str,
) -> Result<(), ManagedStoreLoadError> {
    if entry.byte_length() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || entry.digest() != package_file_digest(bytes)
    {
        return Err(invalid_issue(
            SkillDiagnosticCode::PackageRevisionMismatch,
            format!("{package_location}:{}", entry.path()),
            "Managed Skill package file does not match its revision-bound manifest entry.",
        ));
    }
    Ok(())
}

fn package_validation_issue(
    location: &str,
    error: PackageValidationError,
) -> ManagedStoreLoadError {
    invalid_issue(error.code, location, error.message)
}

fn map_directory_error(location: &str, error: DirectoryCheckError) -> ManagedStoreLoadError {
    match error {
        DirectoryCheckError::Symlink(reason) => {
            invalid_issue(SkillDiagnosticCode::SymlinkNotAllowed, location, reason)
        }
        DirectoryCheckError::EscapesParent(reason) => {
            invalid_issue(SkillDiagnosticCode::RootEscapesWorkspace, location, reason)
        }
        DirectoryCheckError::InvalidComponent(reason)
        | DirectoryCheckError::NotDirectory(reason) => invalid_issue(
            SkillDiagnosticCode::UnexpectedPackageEntry,
            location,
            reason,
        ),
        DirectoryCheckError::Unavailable(reason) => unavailable_issue(location, reason),
    }
}

fn invalid_issue(
    code: SkillDiagnosticCode,
    location: impl Into<String>,
    message: impl Into<String>,
) -> ManagedStoreLoadError {
    ManagedStoreLoadError::Invalid(ManagedStoreIssue::error(code, location, message))
}

fn unavailable_issue(
    location: impl Into<String>,
    message: impl Into<String>,
) -> ManagedStoreLoadError {
    ManagedStoreLoadError::Unavailable(ManagedStoreIssue::error(
        SkillDiagnosticCode::UnreadableEntry,
        location,
        message,
    ))
}

fn display_os_component(component: &OsStr) -> String {
    if let Some(component) = component.to_str() {
        if !component.chars().any(char::is_control) {
            return component.to_string();
        }
    }
    display_os_component_encoded(component)
}

#[cfg(unix)]
fn display_os_component_encoded(component: &OsStr) -> String {
    use std::os::unix::ffi::OsStrExt;
    format!("unix-bytes:{}", percent_encode(component.as_bytes()))
}

#[cfg(windows)]
fn display_os_component_encoded(component: &OsStr) -> String {
    use std::fmt::Write;
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = String::from("windows-wide:");
    for unit in component.encode_wide() {
        write!(&mut encoded, "%u{unit:04X}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(not(any(unix, windows)))]
fn display_os_component_encoded(component: &OsStr) -> String {
    format!("platform-path:{component:?}")
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use serde_json::{json, Value};

    const INSTALLATION_ID: &str = "01234567-89ab-4def-8123-456789abcdef";
    const INSTALLED_AT: u64 = 1_784_347_513_399;

    fn installation_id() -> SkillInstallationId {
        SkillInstallationId::parse(INSTALLATION_ID).unwrap()
    }

    fn package_revision() -> SkillRevision {
        SkillRevision::parse(format!("{PACKAGE_REVISION_PREFIX}{}", "a".repeat(64))).unwrap()
    }

    fn provenance(refresh: Option<&str>) -> SkillInstallationProvenance {
        SkillInstallationProvenance::new(
            SkillInstallationAuthority::new("github", 1, "immutable-commit").unwrap(),
            refresh.map(|payload| SkillInstallationRefresh::new("github", 1, payload).unwrap()),
        )
    }

    fn v1_document() -> Value {
        json!({
            "schemaVersion": 1,
            "installationId": INSTALLATION_ID,
            "package": {
                "formatVersion": 1,
                "revision": package_revision().as_str(),
                "entrypoint": "SKILL.md"
            },
            "origin": {
                "provider": "github",
                "reference": "legacy-audit-reference"
            },
            "installedAtUnixMs": INSTALLED_AT
        })
    }

    fn v2_receipt() -> InstalledSkillReceipt {
        InstalledSkillReceipt::new_v2(
            installation_id(),
            7,
            SKILL_PACKAGE_FORMAT_VERSION,
            package_revision(),
            provenance(Some("named-branch-main")),
            INSTALLED_AT,
            INSTALLED_AT + 1_000,
        )
        .unwrap()
    }

    fn decode_value(value: &Value) -> Result<InstalledSkillReceipt, ManagedStoreLoadError> {
        decode_receipt(&serde_json::to_vec(value).unwrap(), "fixture.json")
    }

    fn assert_invalid(result: Result<InstalledSkillReceipt, ManagedStoreLoadError>) {
        assert!(matches!(result, Err(ManagedStoreLoadError::Invalid(_))));
    }

    #[test]
    fn frozen_v1_receipt_normalizes_without_granting_refresh_authority() {
        let bytes = serde_json::to_vec(&v1_document()).unwrap();
        let first = decode_receipt(&bytes, "fixture.json").unwrap();
        let second = decode_receipt(&bytes, "fixture.json").unwrap();

        assert!(first.is_legacy_v1());
        assert_eq!(first.receipt_schema_version, RECEIPT_SCHEMA_VERSION_V1);
        assert_eq!(first.generation, 1);
        assert_eq!(first.provenance.authority().provider(), "github");
        assert_eq!(
            first.provenance.authority().payload(),
            "legacy-audit-reference"
        );
        assert!(!first.provenance.is_refreshable());
        assert_eq!(first.installed_at_unix_ms, INSTALLED_AT);
        assert_eq!(first.updated_at_unix_ms, INSTALLED_AT);
        assert_eq!(first.installation_revision, second.installation_revision);
        assert!(first
            .installation_revision
            .as_str()
            .starts_with("skill-installation-sha256-v1:"));
    }

    #[test]
    fn v2_writer_round_trips_every_lifecycle_field_and_never_writes_v1() {
        let expected = v2_receipt();
        let bytes = encode_receipt_v2(&expected).unwrap();
        let document: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(document["schemaVersion"], 2);
        assert!(document.get("origin").is_none());
        assert_eq!(document["generation"], 7);
        assert_eq!(
            document["installationRevision"],
            expected.installation_revision.as_str()
        );
        assert_eq!(document["provenance"]["authority"]["provider"], "github");
        assert_eq!(
            document["provenance"]["refresh"]["payload"],
            "named-branch-main"
        );

        let actual = decode_receipt(&bytes, "fixture.json").unwrap();
        assert_eq!(actual.receipt_schema_version, RECEIPT_SCHEMA_VERSION_V2);
        assert_eq!(actual.generation, expected.generation);
        assert_eq!(actual.package.revision, expected.package.revision);
        assert_eq!(actual.provenance, expected.provenance);
        assert_eq!(actual.installation_revision, expected.installation_revision);
        assert_eq!(actual.installed_at_unix_ms, expected.installed_at_unix_ms);
        assert_eq!(actual.updated_at_unix_ms, expected.updated_at_unix_ms);
    }

    #[test]
    fn transitional_encoder_also_emits_only_v2() {
        let origin = SkillPackageOrigin::new("local-directory", "captured-snapshot").unwrap();
        let bytes = encode_receipt(
            &installation_id(),
            SKILL_PACKAGE_FORMAT_VERSION,
            &package_revision(),
            &origin,
            INSTALLED_AT,
        )
        .unwrap();
        let document: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(document["schemaVersion"], RECEIPT_SCHEMA_VERSION_V2);
        assert_eq!(document["generation"], 1);
        assert!(document.get("origin").is_none());
        assert_eq!(document["provenance"]["refresh"], Value::Null);
    }

    #[test]
    fn strict_reader_rejects_unknown_fields_at_every_supported_schema_boundary() {
        let mut v1 = v1_document();
        v1.as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), json!(true));
        assert_invalid(decode_value(&v1));

        let mut v2: Value =
            serde_json::from_slice(&encode_receipt_v2(&v2_receipt()).unwrap()).unwrap();
        v2["provenance"]["authority"]
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), json!(true));
        assert_invalid(decode_value(&v2));
    }

    #[test]
    fn v2_reader_rejects_invalid_generation_timestamps_and_provenance() {
        let encoded = encode_receipt_v2(&v2_receipt()).unwrap();
        let original: Value = serde_json::from_slice(&encoded).unwrap();

        let mut zero_generation = original.clone();
        zero_generation["generation"] = json!(0);
        assert_invalid(decode_value(&zero_generation));

        let mut reversed_time = original.clone();
        reversed_time["updatedAtUnixMs"] = json!(INSTALLED_AT - 1);
        assert_invalid(decode_value(&reversed_time));

        let mut zero_schema = original.clone();
        zero_schema["provenance"]["authority"]["schemaVersion"] = json!(0);
        assert_invalid(decode_value(&zero_schema));

        let mut control_payload = original;
        control_payload["provenance"]["refresh"]["payload"] = json!("branch\nname");
        assert_invalid(decode_value(&control_payload));
    }

    #[test]
    fn v2_reader_recomputes_revision_and_rejects_lifecycle_tampering() {
        let encoded = encode_receipt_v2(&v2_receipt()).unwrap();
        let original: Value = serde_json::from_slice(&encoded).unwrap();

        let mut generation = original.clone();
        generation["generation"] = json!(8);
        assert_invalid(decode_value(&generation));

        let mut authority = original.clone();
        authority["provenance"]["authority"]["payload"] = json!("different-commit");
        assert_invalid(decode_value(&authority));

        let mut refresh = original.clone();
        refresh["provenance"]["refresh"]["payload"] = json!("different-branch");
        assert_invalid(decode_value(&refresh));

        let mut package = original;
        package["package"]["revision"] =
            json!(format!("{PACKAGE_REVISION_PREFIX}{}", "b".repeat(64)));
        assert_invalid(decode_value(&package));
    }

    #[test]
    fn unsupported_receipt_versions_and_malformed_revision_tokens_are_rejected() {
        let mut future = v1_document();
        future["schemaVersion"] = json!(3);
        assert_invalid(decode_value(&future));

        let encoded = encode_receipt_v2(&v2_receipt()).unwrap();
        let mut malformed: Value = serde_json::from_slice(&encoded).unwrap();
        malformed["installationRevision"] =
            json!(format!("skill-installation-sha256-v1:{}", "A".repeat(64)));
        assert_invalid(decode_value(&malformed));
    }
}
