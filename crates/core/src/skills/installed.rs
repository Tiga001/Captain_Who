//! User-installed Skills backed by the read-only managed store.

use super::managed_store::{
    InstalledPackageRef, InstalledSkillReceipt, ManagedPackageSnapshot, ManagedSkillStore,
    ManagedStoreIssue, ManagedStoreLoadError,
};
use super::model::{
    ResolvedSkillPackage, SkillActivationScope, SkillCatalog, SkillDescriptor,
    SkillDescriptorParts, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillId, SkillInstallationId, SkillProvenance, SkillReferenceError,
    SkillRegistrationError, SkillResolveError, SkillSelection, SkillSourceId, SkillSourceKind,
    SkillTrust,
};
use super::prepared::validate_installable_skill_bytes;
use super::resource_runtime::{
    SkillResourceError, SkillResourceReader, SkillResourceReaderRef, SkillResourceSessionBinding,
    SkillResourceSourceError,
};
use super::service::finalize_catalog;
use super::source::SkillSource;
use super::tool_reference_lint::unsupported_model_tool_reference;
use super::workspace::ByteBudget;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const USER_INSTALLED_SKILL_SOURCE_ID: &str = "installed:user";

#[derive(Debug)]
pub(super) struct InstalledSkillSource {
    source_id: SkillSourceId,
    store: ManagedSkillStore,
}

impl InstalledSkillSource {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, SkillRegistrationError> {
        let source_id = SkillSourceId::parse(USER_INSTALLED_SKILL_SOURCE_ID)
            .map_err(invalid_installed_source)?;
        let store = ManagedSkillStore::new(root).map_err(|reason| {
            SkillRegistrationError::InvalidSource {
                reason: format!("invalid managed Skill store: {reason}"),
            }
        })?;
        Ok(Self { source_id, store })
    }

    pub fn source_id(&self) -> &SkillSourceId {
        &self.source_id
    }

    fn load_package(
        &self,
        receipt: &InstalledSkillReceipt,
    ) -> Result<ResolvedSkillPackage, InstalledPackageLoadError> {
        let snapshot = self
            .store
            .load_complete_package(&receipt.package)
            .map_err(|error| store_package_error(error, &receipt.package.relative_path))?;
        build_resolved_package(&self.source_id, receipt, snapshot.package)
            .map_err(InstalledPackageLoadError::Invalid)
    }

    fn load_package_for_catalog(
        &self,
        receipt: &InstalledSkillReceipt,
        byte_budget: &mut ByteBudget,
    ) -> Result<InstalledCatalogPackage, InstalledPackageLoadError> {
        let snapshot = self
            .store
            .load_package_with_budget(&receipt.package, byte_budget)
            .map_err(|error| store_package_error(error, &receipt.package.relative_path))?;
        let mut model_readable_resources = Vec::new();
        for descriptor in snapshot.resources.entries() {
            if matches!(
                descriptor.kind(),
                super::model::SkillResourceKind::Asset | super::model::SkillResourceKind::Script
            ) {
                continue;
            }
            // Resource lint is advisory catalog metadata. Catalog eligibility is
            // intentionally manifest-only, so a resource that cannot be read or
            // verified here must not hide an otherwise valid descriptor. The
            // authoritative resolve path still loads and verifies every
            // revision-bound resource before activation.
            let Ok(bytes) = self.store.load_resource(&receipt.package, descriptor) else {
                continue;
            };
            if std::str::from_utf8(&bytes).is_ok() {
                model_readable_resources.push((descriptor.path().to_string(), bytes));
            }
        }
        let package = build_resolved_package(&self.source_id, receipt, snapshot)
            .map_err(InstalledPackageLoadError::Invalid)?;
        Ok(InstalledCatalogPackage {
            package,
            model_readable_resources,
        })
    }

    #[cfg(test)]
    fn set_package_catalog_byte_limit(&mut self, max_bytes: usize) {
        self.store.set_package_catalog_byte_limit(max_bytes);
    }

    #[cfg(test)]
    fn set_installation_entry_limit(&mut self, max_entries: usize) {
        self.store.set_installation_entry_limit(max_entries);
    }

    #[cfg(test)]
    fn set_installation_directory_entry_limit(&mut self, max_entries: usize) {
        self.store
            .set_installation_directory_entry_limit(max_entries);
    }

    #[cfg(test)]
    fn set_receipt_byte_limit(&mut self, max_bytes: usize) {
        self.store.set_receipt_byte_limit(max_bytes);
    }

    #[cfg(test)]
    fn set_receipt_catalog_byte_limit(&mut self, max_bytes: usize) {
        self.store.set_receipt_catalog_byte_limit(max_bytes);
    }
}

impl SkillSource for InstalledSkillSource {
    fn id(&self) -> &SkillSourceId {
        self.source_id()
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Installed
    }

    fn trust(&self) -> SkillTrust {
        SkillTrust::Untrusted
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        let index =
            self.store
                .scan_receipts()
                .map_err(|error| SkillDiscoveryError::InvalidSource {
                    reason: format!("managed Skill store is unavailable: {error}"),
                })?;
        let mut diagnostics = index
            .issues
            .into_iter()
            .map(issue_diagnostic)
            .collect::<Vec<_>>();
        let mut skills = Vec::with_capacity(index.receipts.len());
        let mut truncated = index.truncated;
        let mut package_budget = self.store.package_catalog_budget();
        for receipt in index.receipts {
            match self.load_package_for_catalog(&receipt, &mut package_budget) {
                Ok(loaded) => {
                    let package = loaded.package;
                    let location = match package.descriptor().provenance() {
                        SkillProvenance::Installed { relative_path, .. } => relative_path.clone(),
                        _ => "installations".to_string(),
                    };
                    if let Some(tool_name) =
                        unsupported_model_tool_reference(package.instructions())
                    {
                        diagnostics.push(SkillDiagnostic::new(
                            SkillDiagnosticCode::UnsupportedToolReference,
                            SkillDiagnosticSeverity::Warning,
                            format!(
                                "Skill instructions reference unsupported model tool `{tool_name}`; update the Skill to use `apply_patch`. The installed package was not rewritten."
                            ),
                            location.clone(),
                        ));
                    }
                    for (resource_path, bytes) in loaded.model_readable_resources {
                        let text = std::str::from_utf8(&bytes)
                            .expect("catalog resource collection retains only UTF-8 bytes");
                        let Some(tool_name) = unsupported_model_tool_reference(text) else {
                            continue;
                        };
                        diagnostics.push(SkillDiagnostic::new(
                            SkillDiagnosticCode::UnsupportedToolReference,
                            SkillDiagnosticSeverity::Warning,
                            format!(
                                "Skill resource references unsupported model tool `{tool_name}`; update the Skill to use `apply_patch`. The installed package was not rewritten."
                            ),
                            format!("{location}/{resource_path}"),
                        ));
                    }
                    skills.push(package.descriptor().clone());
                }
                Err(error) => {
                    let issue = error.into_issue();
                    if issue.code == SkillDiagnosticCode::CatalogTooLarge {
                        diagnostics.push(issue_diagnostic(issue));
                        truncated = true;
                        break;
                    }
                    diagnostics.push(issue_diagnostic(issue));
                }
            }
        }
        append_duplicate_name_diagnostics(&skills, &mut diagnostics);
        Ok(finalize_catalog(skills, diagnostics, truncated))
    }

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        if selection.skill_id().source_id() != &self.source_id {
            return Err(SkillResolveError::InvalidReference {
                reason: format!(
                    "Skill `{}` does not belong to source `{}`",
                    selection.skill_id(),
                    self.source_id
                ),
            });
        }
        let installation_id =
            SkillInstallationId::parse(selection.skill_id().local_id()).map_err(|error| {
                SkillResolveError::InvalidReference {
                    reason: format!("invalid installed Skill id: {error}"),
                }
            })?;
        let receipt = self
            .store
            .load_receipt(&installation_id)
            .map_err(|error| resolve_store_error(selection, error))?;

        // Receipt replacement is the update linearization point. Report a
        // stale selection before reading or validating the newly referenced
        // package, matching the workspace source's revision semantics.
        if receipt.package.revision != *selection.expected_revision() {
            return Err(SkillResolveError::Stale {
                skill_id: selection.skill_id().clone(),
                expected_revision: selection.expected_revision().clone(),
                actual_revision: receipt.package.revision,
            });
        }

        self.load_package(&receipt).map_err(|error| match error {
            InstalledPackageLoadError::Invalid(issue) => SkillResolveError::InvalidSkill {
                skill_id: selection.skill_id().clone(),
                code: issue.code,
                reason: issue.message,
            },
            InstalledPackageLoadError::Unavailable(issue) => SkillResolveError::Unavailable {
                skill_id: Some(selection.skill_id().clone()),
                reason: issue.message,
            },
        })
    }

    fn open_resource_reader(
        &self,
        package: &ResolvedSkillPackage,
    ) -> Result<Option<SkillResourceReaderRef>, SkillResourceError> {
        if package.id().source_id() != &self.source_id {
            return Err(SkillResourceError::SourceContractViolation {
                source_id: self.source_id.clone(),
                reason: format!(
                    "Skill `{}` does not belong to this installed source",
                    package.id()
                ),
            });
        }
        if package.resources().is_empty() {
            return Ok(None);
        }
        let package_ref = InstalledPackageRef::from_format_and_revision(
            package.format_version(),
            package.revision().clone(),
        )
        .map_err(|reason| SkillResourceError::SourceContractViolation {
            source_id: self.source_id.clone(),
            reason,
        })?;
        Ok(Some(Arc::new(InstalledSkillResourceReader {
            store: self.store.clone(),
            package: package_ref,
        })))
    }

    fn restore_resource_binding(
        &self,
        selection: &SkillSelection,
    ) -> Result<SkillResourceSessionBinding, SkillResourceError> {
        if selection.skill_id().source_id() != &self.source_id {
            return Err(SkillResourceError::SourceContractViolation {
                source_id: self.source_id.clone(),
                reason: format!(
                    "Skill `{}` does not belong to this installed source",
                    selection.skill_id()
                ),
            });
        }
        SkillInstallationId::parse(selection.skill_id().local_id()).map_err(|error| {
            SkillResourceError::SnapshotUnavailable {
                skill_id: selection.skill_id().clone(),
                revision: selection.expected_revision().clone(),
                reason: format!("invalid installed Skill id: {error}"),
            }
        })?;
        let package_ref = InstalledPackageRef::from_revision(selection.expected_revision().clone())
            .map_err(|reason| SkillResourceError::SnapshotUnavailable {
                skill_id: selection.skill_id().clone(),
                revision: selection.expected_revision().clone(),
                reason,
            })?;
        let snapshot = self
            .store
            .load_indexed_package(&package_ref)
            .map_err(|error| restore_store_error(selection, error))?;
        let reader = (!snapshot.resources.is_empty()).then(|| {
            Arc::new(InstalledSkillResourceReader {
                store: self.store.clone(),
                package: package_ref,
            }) as SkillResourceReaderRef
        });
        Ok(SkillResourceSessionBinding {
            skill_id: selection.skill_id().clone(),
            revision: selection.expected_revision().clone(),
            source_id: self.source_id.clone(),
            source_kind: SkillSourceKind::Installed,
            trust: SkillTrust::Untrusted,
            resources: snapshot.resources,
            reader,
        })
    }
}

struct InstalledCatalogPackage {
    package: ResolvedSkillPackage,
    model_readable_resources: Vec<(String, Vec<u8>)>,
}

struct InstalledSkillResourceReader {
    store: ManagedSkillStore,
    package: InstalledPackageRef,
}

impl SkillResourceReader for InstalledSkillResourceReader {
    fn read(
        &self,
        expected: &super::model::SkillResourceDescriptor,
    ) -> Result<Vec<u8>, SkillResourceSourceError> {
        self.store
            .load_resource(&self.package, expected)
            .map_err(|error| match error {
                ManagedStoreLoadError::NotFound => SkillResourceSourceError::Unavailable(
                    "the immutable package resource is no longer present".to_string(),
                ),
                ManagedStoreLoadError::Invalid(issue) => SkillResourceSourceError::Integrity(
                    format!("{}: {}", issue.code.stable_name(), issue.message),
                ),
                ManagedStoreLoadError::Unavailable(issue) => {
                    SkillResourceSourceError::Unavailable(issue.message)
                }
            })
    }
}

fn restore_store_error(
    selection: &SkillSelection,
    error: ManagedStoreLoadError,
) -> SkillResourceError {
    match error {
        ManagedStoreLoadError::NotFound => SkillResourceError::SnapshotUnavailable {
            skill_id: selection.skill_id().clone(),
            revision: selection.expected_revision().clone(),
            reason: "the immutable managed package is no longer present".to_string(),
        },
        ManagedStoreLoadError::Invalid(issue) => SkillResourceError::SnapshotIntegrityMismatch {
            skill_id: selection.skill_id().clone(),
            revision: selection.expected_revision().clone(),
            reason: format!("{}: {}", issue.code.stable_name(), issue.message),
        },
        ManagedStoreLoadError::Unavailable(issue) => SkillResourceError::SnapshotUnavailable {
            skill_id: selection.skill_id().clone(),
            revision: selection.expected_revision().clone(),
            reason: issue.message,
        },
    }
}

fn build_resolved_package(
    source_id: &SkillSourceId,
    receipt: &InstalledSkillReceipt,
    snapshot: ManagedPackageSnapshot,
) -> Result<ResolvedSkillPackage, ManagedStoreIssue> {
    let location = snapshot.relative_path;
    let document = validate_installable_skill_bytes(snapshot.bytes)
        .map_err(|error| ManagedStoreIssue::error(error.code, &location, error.message))?;
    let skill_id = SkillId::from_parts(source_id.clone(), receipt.installation_id.as_str())
        .map_err(|error| {
            ManagedStoreIssue::error(
                SkillDiagnosticCode::InvalidInstallationReceipt,
                &location,
                format!("Installation id cannot form a Skill id: {error}"),
            )
        })?;
    let descriptor = SkillDescriptor::new(SkillDescriptorParts {
        id: skill_id,
        name: document.name,
        description: document.description,
        source_kind: SkillSourceKind::Installed,
        trust: SkillTrust::Untrusted,
        activation_scope: SkillActivationScope::Run,
        revision: receipt.package.revision.clone(),
        provenance: SkillProvenance::Installed {
            source_id: source_id.clone(),
            installation_id: receipt.installation_id.clone(),
            relative_path: location.clone(),
        },
    });
    ResolvedSkillPackage::with_resources(
        descriptor,
        snapshot.format_version,
        snapshot.resources,
        document.source,
        document.instructions_range,
    )
    .map_err(|error| {
        ManagedStoreIssue::error(
            SkillDiagnosticCode::SourceContractViolation,
            location,
            format!("Cannot construct verified managed Skill snapshot: {error}"),
        )
    })
}

enum InstalledPackageLoadError {
    Invalid(ManagedStoreIssue),
    Unavailable(ManagedStoreIssue),
}

impl InstalledPackageLoadError {
    fn into_issue(self) -> ManagedStoreIssue {
        match self {
            Self::Invalid(issue) | Self::Unavailable(issue) => issue,
        }
    }
}

fn store_package_error(error: ManagedStoreLoadError, location: &str) -> InstalledPackageLoadError {
    match error {
        ManagedStoreLoadError::NotFound => {
            InstalledPackageLoadError::Invalid(ManagedStoreIssue::error(
                SkillDiagnosticCode::MissingSkillFile,
                location,
                "Managed Skill package is missing.",
            ))
        }
        ManagedStoreLoadError::Invalid(issue) => InstalledPackageLoadError::Invalid(issue),
        ManagedStoreLoadError::Unavailable(issue) => InstalledPackageLoadError::Unavailable(issue),
    }
}

fn resolve_store_error(
    selection: &SkillSelection,
    error: ManagedStoreLoadError,
) -> SkillResolveError {
    match error {
        ManagedStoreLoadError::NotFound => SkillResolveError::NotFound {
            skill_id: selection.skill_id().clone(),
        },
        ManagedStoreLoadError::Invalid(issue) => SkillResolveError::InvalidSkill {
            skill_id: selection.skill_id().clone(),
            code: issue.code,
            reason: issue.message,
        },
        ManagedStoreLoadError::Unavailable(issue) => SkillResolveError::Unavailable {
            skill_id: Some(selection.skill_id().clone()),
            reason: issue.message,
        },
    }
}

fn issue_diagnostic(issue: ManagedStoreIssue) -> SkillDiagnostic {
    SkillDiagnostic::new(issue.code, issue.severity, issue.message, issue.location)
}

fn append_duplicate_name_diagnostics(
    skills: &[SkillDescriptor],
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let mut receipts_by_name = BTreeMap::<&str, Vec<String>>::new();
    for skill in skills {
        let SkillProvenance::Installed {
            installation_id, ..
        } = skill.provenance()
        else {
            continue;
        };
        receipts_by_name
            .entry(skill.name())
            .or_default()
            .push(format!("installations/{installation_id}.json"));
    }
    for (name, receipts) in receipts_by_name {
        if receipts.len() < 2 {
            continue;
        }
        diagnostics.push(SkillDiagnostic::new(
            SkillDiagnosticCode::DuplicateName,
            SkillDiagnosticSeverity::Warning,
            format!(
                "Installed Skill name `{name}` is declared by multiple receipts: {}. Explicit selection must use Skill id.",
                receipts.join(", ")
            ),
            "installations".to_string(),
        ));
    }
}

fn invalid_installed_source(error: SkillReferenceError) -> SkillRegistrationError {
    SkillRegistrationError::InvalidSource {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
