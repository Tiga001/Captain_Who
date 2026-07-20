use super::bundled::BundledSkillSource;
use super::digest::{activation_revision, catalog_revision, package_revision};
use super::installed::InstalledSkillSource;
use super::managed_store::managed_package_relative_path;
use super::model::{
    ActivatedSkillSet, ResolvedSkillPackage, SkillActivationError, SkillActivationPolicy,
    SkillCatalog, SkillDescriptor, SkillDiagnostic, SkillDiagnosticCode, SkillDiagnosticSeverity,
    SkillDiscoveryError, SkillProvenance, SkillRegistrationError, SkillResolveError,
    SkillResolveRequest, SkillSelection, SkillSourceId, SkillSourceKind,
    SKILL_PACKAGE_FORMAT_VERSION, SKILL_PACKAGE_FORMAT_VERSION_V2, SKILL_PACKAGE_FORMAT_VERSION_V3,
};
use super::package::PackageManifest;
use super::parser::parse_skill_document;
use super::resource_runtime::{
    SkillResourceError, SkillResourceSession, SkillResourceSessionBinding,
};
use super::source::{SkillSource, WorkspaceSkillSource};
use super::workspace::{
    percent_encode, AGENTS_DIRECTORY, MAX_SKILL_FILE_BYTES, SKILLS_DIRECTORY, SKILL_FILE_NAME,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod validation;

use validation::*;

pub struct SkillsService {
    registry: BTreeMap<SkillSourceId, Arc<dyn SkillSource>>,
    activation_policy: SkillActivationPolicy,
}

impl SkillsService {
    pub fn new() -> Self {
        Self {
            registry: BTreeMap::new(),
            activation_policy: SkillActivationPolicy::default(),
        }
    }

    pub fn for_workspace(
        workspace_id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, SkillRegistrationError> {
        Self::new().with_workspace_source(workspace_id, workspace_root)
    }

    pub fn with_workspace_source(
        mut self,
        workspace_id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, SkillRegistrationError> {
        let source = Arc::new(WorkspaceSkillSource::new(workspace_id, workspace_root)?);
        self.register_source(source)?;
        Ok(self)
    }

    /// Register the application-owned, compile-time embedded Skill source.
    pub fn with_bundled_source(mut self) -> Result<Self, SkillRegistrationError> {
        let source = Arc::new(BundledSkillSource::new()?);
        self.register_source(source)?;
        Ok(self)
    }

    /// Register the read-only user-installed source backed by an
    /// application-managed, content-addressed store.
    ///
    /// `managed_root` must be absolute. Registration records configuration
    /// only and never creates directories; a missing root lists as an empty
    /// installed catalog until a future installer atomically publishes it.
    pub fn with_installed_source(
        mut self,
        managed_root: impl Into<PathBuf>,
    ) -> Result<Self, SkillRegistrationError> {
        let source = Arc::new(InstalledSkillSource::new(managed_root)?);
        self.register_source(source)?;
        Ok(self)
    }

    pub fn with_activation_policy(mut self, policy: SkillActivationPolicy) -> Self {
        self.activation_policy = policy;
        self
    }

    pub fn activation_policy(&self) -> SkillActivationPolicy {
        self.activation_policy
    }

    /// List every source registered in this service instance.
    pub fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        Ok(self.aggregate_catalog())
    }

    fn aggregate_catalog(&self) -> SkillCatalog {
        let mut skills = Vec::new();
        let mut diagnostics = Vec::new();
        let mut truncated = false;
        let mut catalog_ids = BTreeSet::new();
        for source in self.registry.values() {
            let catalog = match source.list() {
                Ok(catalog) => catalog,
                Err(error) => {
                    diagnostics.push(source_diagnostic(
                        source.id(),
                        SkillDiagnosticCode::SourceUnavailable,
                        error.message(),
                    ));
                    continue;
                }
            };
            let (source_skills, source_diagnostics, source_truncated) = catalog.into_parts();
            diagnostics.extend(source_diagnostics);
            truncated |= source_truncated;
            for descriptor in source_skills {
                if let Err(reason) = validate_descriptor_contract(source.as_ref(), &descriptor) {
                    diagnostics.push(source_diagnostic(
                        source.id(),
                        SkillDiagnosticCode::SourceContractViolation,
                        reason,
                    ));
                    continue;
                }
                if !catalog_ids.insert(descriptor.id().clone()) {
                    diagnostics.push(source_diagnostic(
                        source.id(),
                        SkillDiagnosticCode::SourceContractViolation,
                        format!(
                            "Skill source returned duplicate descriptor id `{}`",
                            descriptor.id()
                        ),
                    ));
                    continue;
                }
                skills.push(descriptor);
            }
        }
        finalize_catalog(skills, diagnostics, truncated)
    }

    /// Route one immutable selection to its owning source.
    pub fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        let source_id = selection.skill_id().source_id();
        let source =
            self.registry
                .get(source_id)
                .ok_or_else(|| SkillResolveError::SourceNotRegistered {
                    source_id: source_id.clone(),
                })?;
        let package = source.resolve(selection)?;
        validate_resolved_contract(source.as_ref(), selection, &package).map_err(|reason| {
            SkillResolveError::SourceContractViolation {
                source_id: source.id().clone(),
                skill_id: Some(selection.skill_id().clone()),
                reason,
            }
        })?;
        Ok(package)
    }

    /// Resolve a stable, duplicate-free set atomically and enforce aggregate
    /// activation limits before returning any snapshot to the caller.
    pub fn activate(
        &self,
        selections: &[SkillSelection],
    ) -> Result<ActivatedSkillSet, SkillActivationError> {
        let unique = validate_unique_selections(selections)?;
        if unique.len() > self.activation_policy.max_skills() {
            return Err(SkillActivationError::TooManySkills {
                max: self.activation_policy.max_skills(),
                actual: unique.len(),
            });
        }

        let mut resolved = Vec::with_capacity(unique.len());
        let mut total_source_bytes = 0usize;
        for (selection_index, selection) in unique {
            let package =
                self.resolve(selection)
                    .map_err(|source| SkillActivationError::Resolve {
                        selection_index,
                        source,
                    })?;
            total_source_bytes = total_source_bytes.saturating_add(package.source_text().len());
            if total_source_bytes > self.activation_policy.max_total_source_bytes() {
                return Err(SkillActivationError::SourceBudgetExceeded {
                    max_bytes: self.activation_policy.max_total_source_bytes(),
                    actual_bytes: total_source_bytes,
                });
            }
            resolved.push(package);
        }

        let revision = activation_revision(resolved.iter().map(ResolvedSkillPackage::descriptor));
        Ok(ActivatedSkillSet::new(
            resolved,
            revision,
            total_source_bytes,
        ))
    }

    /// Create immutable resource grants for an already activated Skill set.
    ///
    /// The returned session captures only exact package ids, revisions,
    /// descriptors, and source-owned readers. It never stores resource bytes
    /// in Agent input and never follows a later installation receipt.
    pub fn resource_session(
        &self,
        activated: &ActivatedSkillSet,
    ) -> Result<SkillResourceSession, SkillResourceError> {
        let mut bindings = Vec::with_capacity(activated.skills().len());
        for package in activated.skills() {
            let source_id = package.id().source_id();
            // Instruction-only packages need no source-owned byte authority.
            // This also keeps request-scoped workspace Skills usable after
            // their temporary source has finished activation. If a future
            // workspace package exposes siblings, it must take the normal
            // reader path below and remain bound to that scoped source.
            if package.resources().is_empty() {
                bindings.push(SkillResourceSessionBinding {
                    skill_id: package.id().clone(),
                    revision: package.revision().clone(),
                    source_id: source_id.clone(),
                    resources: package.resources().clone(),
                    reader: None,
                });
                continue;
            }
            let source = self.registry.get(source_id).ok_or_else(|| {
                SkillResourceError::SnapshotUnavailable {
                    skill_id: package.id().clone(),
                    revision: package.revision().clone(),
                    reason: format!("Skill source `{source_id}` is not registered"),
                }
            })?;
            validate_descriptor_contract(source.as_ref(), package.descriptor()).map_err(
                |reason| SkillResourceError::SourceContractViolation {
                    source_id: source_id.clone(),
                    reason,
                },
            )?;
            let reader = source.open_resource_reader(package)?;
            bindings.push(SkillResourceSessionBinding {
                skill_id: package.id().clone(),
                revision: package.revision().clone(),
                source_id: source_id.clone(),
                resources: package.resources().clone(),
                reader,
            });
        }
        SkillResourceSession::from_bindings(bindings)
    }

    /// Restore run-scoped resource grants from persisted activated id +
    /// revision metadata, such as a pending approval after process restart.
    ///
    /// Installed sources reopen the immutable content-addressed revision
    /// directly. The current receipt is deliberately not consulted, so an
    /// update or uninstall cannot redirect a restored grant to different
    /// bytes.
    pub fn restore_resource_session(
        &self,
        selections: &[SkillSelection],
    ) -> Result<SkillResourceSession, SkillResourceError> {
        if selections.len() > self.activation_policy.max_skills() {
            return Err(SkillResourceError::InvalidRequest {
                reason: format!(
                    "cannot restore {} Skill resource bindings; the limit is {}",
                    selections.len(),
                    self.activation_policy.max_skills()
                ),
            });
        }
        let mut seen = BTreeSet::new();
        let mut bindings = Vec::with_capacity(selections.len());
        for selection in selections {
            if !seen.insert(selection.skill_id().clone()) {
                return Err(SkillResourceError::InvalidRequest {
                    reason: format!(
                        "Skill `{}` appears more than once in restored activation metadata",
                        selection.skill_id()
                    ),
                });
            }
            let source_id = selection.skill_id().source_id();
            let source = self.registry.get(source_id).ok_or_else(|| {
                SkillResourceError::SnapshotUnavailable {
                    skill_id: selection.skill_id().clone(),
                    revision: selection.expected_revision().clone(),
                    reason: format!("Skill source `{source_id}` is not registered"),
                }
            })?;
            let binding = source.restore_resource_binding(selection)?;
            if binding.skill_id != *selection.skill_id()
                || binding.revision != *selection.expected_revision()
                || binding.source_id != *source_id
            {
                return Err(SkillResourceError::SourceContractViolation {
                    source_id: source_id.clone(),
                    reason: "source restored a different Skill id or revision".to_string(),
                });
            }
            bindings.push(binding);
        }
        SkillResourceSession::from_bindings(bindings)
    }

    /// Compatibility adapter for request-scoped workspace lookup.
    pub fn list_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
    ) -> Result<SkillCatalog, SkillDiscoveryError> {
        let source = WorkspaceSkillSource::new(workspace_id, workspace_root).map_err(|error| {
            SkillDiscoveryError::InvalidSource {
                reason: error.to_string(),
            }
        })?;
        source.list()
    }

    /// Aggregate registered sources with one request-scoped workspace source.
    ///
    /// This is the production catalog path when the workspace is known only
    /// after resolving a project id. Existing registered sources remain
    /// immutable; a colliding workspace source id is rejected explicitly.
    pub fn list_with_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
    ) -> Result<SkillCatalog, SkillRegistrationError> {
        Ok(self
            .with_request_workspace(workspace_id, workspace_root)?
            .aggregate_catalog())
    }

    /// Compatibility adapter for one request-scoped workspace selection.
    pub fn resolve_workspace_skill(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
        request: &SkillResolveRequest,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        let source = WorkspaceSkillSource::new(workspace_id, workspace_root).map_err(|error| {
            SkillResolveError::InvalidReference {
                reason: error.to_string(),
            }
        })?;
        let package = source.resolve(request)?;
        validate_resolved_contract(&source, request, &package).map_err(|reason| {
            SkillResolveError::SourceContractViolation {
                source_id: source.id().clone(),
                skill_id: Some(request.skill_id().clone()),
                reason,
            }
        })?;
        Ok(package)
    }

    /// Request-scoped production entry point when the workspace is known only
    /// after resolving a project id.
    pub fn activate_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
        selections: &[SkillSelection],
    ) -> Result<ActivatedSkillSet, SkillActivationError> {
        let service = self
            .with_request_workspace(workspace_id, workspace_root)
            .map_err(|source| SkillActivationError::SourceRegistration { source })?;
        service.activate(selections)
    }

    #[cfg(test)]
    pub(super) fn from_sources(
        sources: impl IntoIterator<Item = Arc<dyn SkillSource>>,
    ) -> Result<Self, SkillRegistrationError> {
        let mut service = Self::new();
        for source in sources {
            service.register_source(source)?;
        }
        Ok(service)
    }

    fn register_source(
        &mut self,
        source: Arc<dyn SkillSource>,
    ) -> Result<(), SkillRegistrationError> {
        let source_id = source.id().clone();
        let declared_kind = source_id
            .as_str()
            .split_once(':')
            .map(|(kind, _)| kind)
            .unwrap_or_default();
        if declared_kind != source.kind().stable_name() {
            return Err(SkillRegistrationError::InvalidSource {
                reason: format!(
                    "Skill source id `{source_id}` declares kind `{declared_kind}` but the source reports kind `{}`",
                    source.kind().stable_name()
                ),
            });
        }
        if self.registry.contains_key(&source_id) {
            return Err(SkillRegistrationError::DuplicateSource { source_id });
        }
        self.registry.insert(source_id, source);
        Ok(())
    }

    fn with_request_workspace(
        &self,
        workspace_id: &str,
        workspace_root: &Path,
    ) -> Result<Self, SkillRegistrationError> {
        let mut service = Self {
            registry: self.registry.clone(),
            activation_policy: self.activation_policy,
        };
        let source = Arc::new(WorkspaceSkillSource::new(workspace_id, workspace_root)?);
        service.register_source(source)?;
        Ok(service)
    }
}

impl Default for SkillsService {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillsService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillsService")
            .field("source_ids", &self.registry.keys().collect::<Vec<_>>())
            .field("activation_policy", &self.activation_policy)
            .finish()
    }
}

pub(super) fn finalize_catalog(
    mut skills: Vec<SkillDescriptor>,
    mut diagnostics: Vec<SkillDiagnostic>,
    truncated: bool,
) -> SkillCatalog {
    skills.sort_by(|left, right| {
        left.name()
            .cmp(right.name())
            .then_with(|| left.id().cmp(right.id()))
    });
    diagnostics.sort_by(|left, right| {
        left.path()
            .cmp(right.path())
            .then_with(|| left.code().stable_name().cmp(right.code().stable_name()))
            .then_with(|| left.message().cmp(right.message()))
    });
    let revision = catalog_revision(&skills, &diagnostics, truncated);
    SkillCatalog::new(revision, skills, diagnostics, truncated)
}

#[cfg(test)]
mod tests;
