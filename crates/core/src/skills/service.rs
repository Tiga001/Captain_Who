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
use super::source::{SkillSource, WorkspaceSkillSource};
use super::workspace::{
    percent_encode, AGENTS_DIRECTORY, MAX_SKILL_FILE_BYTES, SKILLS_DIRECTORY, SKILL_FILE_NAME,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

fn validate_descriptor_contract(
    source: &dyn SkillSource,
    descriptor: &SkillDescriptor,
) -> Result<(), String> {
    if descriptor.id().source_id() != source.id() {
        return Err(format!(
            "descriptor `{}` belongs to source `{}` instead of registered source `{}`",
            descriptor.id(),
            descriptor.id().source_id(),
            source.id()
        ));
    }
    if descriptor.source_kind() != source.kind() {
        return Err(format!(
            "descriptor `{}` reports source kind `{}` instead of `{}`",
            descriptor.id(),
            descriptor.source_kind().stable_name(),
            source.kind().stable_name()
        ));
    }
    if descriptor.trust() != source.trust() {
        return Err(format!(
            "descriptor `{}` reports trust `{}` instead of `{}`",
            descriptor.id(),
            descriptor.trust().stable_name(),
            source.trust().stable_name()
        ));
    }
    if descriptor.activation_scope() != source.activation_scope() {
        return Err(format!(
            "descriptor `{}` reports activation scope `{}` instead of `{}`",
            descriptor.id(),
            descriptor.activation_scope().stable_name(),
            source.activation_scope().stable_name()
        ));
    }
    match (source.kind(), descriptor.provenance()) {
        (
            SkillSourceKind::Workspace,
            SkillProvenance::Workspace {
                workspace_id,
                relative_path,
            },
        ) => {
            let expected = format!("workspace:{}", percent_encode(workspace_id.as_bytes()));
            if expected != source.id().as_str() {
                return Err(format!(
                    "descriptor `{}` has workspace provenance for a different source",
                    descriptor.id()
                ));
            }
            validate_workspace_provenance_path(descriptor, relative_path)?;
        }
        (
            SkillSourceKind::Bundled,
            SkillProvenance::Bundled {
                source_id,
                relative_path,
            },
        ) if source_id == source.id() => {
            if !is_canonical_display_path(relative_path) {
                return Err(format!(
                    "descriptor `{}` has a non-canonical bundled path `{relative_path}`",
                    descriptor.id()
                ));
            }
            let expected = format!("{}/SKILL.md", descriptor.id().local_id());
            if relative_path != &expected {
                return Err(format!(
                    "descriptor `{}` has bundled path `{relative_path}` instead of `{expected}`",
                    descriptor.id()
                ));
            }
        }
        (
            SkillSourceKind::Installed,
            SkillProvenance::Installed {
                source_id,
                installation_id,
                relative_path,
            },
        ) if source_id == source.id() => {
            if descriptor.id().local_id() != installation_id.as_str() {
                return Err(format!(
                    "descriptor `{}` is not bound to installation `{installation_id}`",
                    descriptor.id()
                ));
            }
            if !is_canonical_display_path(relative_path) {
                return Err(format!(
                    "descriptor `{}` has a non-canonical installed package path `{relative_path}`",
                    descriptor.id()
                ));
            }
            let expected =
                managed_package_relative_path(descriptor.revision()).map_err(|reason| {
                    format!(
                        "descriptor `{}` has an invalid managed package revision: {reason}",
                        descriptor.id()
                    )
                })?;
            if relative_path != &expected {
                return Err(format!(
                    "descriptor `{}` has installed package path `{relative_path}` instead of `{expected}`",
                    descriptor.id()
                ));
            }
        }
        _ => {
            return Err(format!(
                "descriptor `{}` has provenance incompatible with source kind `{}`",
                descriptor.id(),
                source.kind().stable_name()
            ));
        }
    }
    Ok(())
}

fn validate_workspace_provenance_path(
    descriptor: &SkillDescriptor,
    relative_path: &str,
) -> Result<(), String> {
    if !is_canonical_display_path(relative_path) {
        return Err(format!(
            "descriptor `{}` has a non-canonical workspace path `{relative_path}`",
            descriptor.id()
        ));
    }
    let components = relative_path.split('/').collect::<Vec<_>>();
    let [agents, skills, directory, skill_file] = components.as_slice() else {
        return Err(format!(
            "descriptor `{}` has workspace path `{relative_path}` outside the supported Skill layout",
            descriptor.id()
        ));
    };
    if *agents != AGENTS_DIRECTORY || *skills != SKILLS_DIRECTORY || *skill_file != SKILL_FILE_NAME
    {
        return Err(format!(
            "descriptor `{}` has workspace path `{relative_path}` outside the supported Skill layout",
            descriptor.id()
        ));
    }
    let expected_local_id = percent_encode(directory.as_bytes());
    if descriptor.id().local_id() != expected_local_id {
        return Err(format!(
            "descriptor `{}` is not bound to workspace directory `{directory}`",
            descriptor.id()
        ));
    }
    Ok(())
}

fn is_canonical_display_path(relative_path: &str) -> bool {
    !relative_path.is_empty()
        && !relative_path.starts_with('/')
        && !relative_path.contains('\\')
        && !relative_path.chars().any(char::is_control)
        && relative_path
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

fn validate_resolved_contract(
    source: &dyn SkillSource,
    selection: &SkillSelection,
    package: &ResolvedSkillPackage,
) -> Result<(), String> {
    validate_descriptor_contract(source, package.descriptor())?;
    if package.source_text().len() > MAX_SKILL_FILE_BYTES {
        return Err(format!(
            "resolved package source exceeds the {MAX_SKILL_FILE_BYTES}-byte limit"
        ));
    }
    let actual_revision = match package.format_version() {
        SKILL_PACKAGE_FORMAT_VERSION => package_revision(package.source_text().as_bytes()),
        SKILL_PACKAGE_FORMAT_VERSION_V2 | SKILL_PACKAGE_FORMAT_VERSION_V3 => {
            let manifest = PackageManifest::from_resolved(
                package.source_text().as_bytes(),
                package.resources(),
            )
            .map_err(|error| {
                format!(
                    "resolved package `{}` has an invalid resource index: {}",
                    package.id(),
                    error.message
                )
            })?;
            if manifest.format_version() != package.format_version() {
                return Err(format!(
                    "resolved package `{}` declares format {}, but its resource index requires format {}",
                    package.id(),
                    package.format_version(),
                    manifest.format_version()
                ));
            }
            manifest.revision()
        }
        version => {
            return Err(format!(
                "resolved package `{}` uses unsupported format {version}",
                package.id()
            ))
        }
    };
    if package.revision() != &actual_revision {
        return Err(format!(
            "resolved package revision `{}` does not match its source snapshot `{actual_revision}`",
            package.revision()
        ));
    }
    let default_name = match package.provenance() {
        SkillProvenance::Workspace { relative_path, .. } => {
            relative_path.split('/').nth(2).ok_or_else(|| {
                format!(
                    "resolved package `{}` has no workspace directory",
                    package.id()
                )
            })?
        }
        SkillProvenance::Bundled { .. } => package.id().local_id(),
        SkillProvenance::Installed {
            installation_id, ..
        } => installation_id.as_str(),
        SkillProvenance::Other { .. } => {
            return Err(format!(
                "resolved package `{}` has unsupported provenance",
                package.id()
            ));
        }
    };
    let parsed = parse_skill_document(package.source_text(), default_name).map_err(|error| {
        format!(
            "resolved package source cannot be parsed as its descriptor `{}`: {error}",
            package.id()
        )
    })?;
    if matches!(
        source.kind(),
        SkillSourceKind::Bundled | SkillSourceKind::Installed
    ) && parsed.metadata.name_was_defaulted
    {
        return Err(format!(
            "resolved non-workspace package `{}` must declare an explicit name",
            package.id()
        ));
    }
    if parsed.metadata.name != package.name() {
        return Err(format!(
            "resolved package metadata name `{}` does not match descriptor name `{}`",
            parsed.metadata.name,
            package.name()
        ));
    }
    if parsed.metadata.description != package.description() {
        return Err(format!(
            "resolved package metadata description does not match descriptor `{}`",
            package.id()
        ));
    }
    if &parsed.instructions_range != package.instructions_range() {
        return Err(format!(
            "resolved package instruction range does not match its parsed source for `{}`",
            package.id()
        ));
    }
    if package.id() != selection.skill_id() {
        return Err(format!(
            "resolved package id `{}` does not match selection `{}`",
            package.id(),
            selection.skill_id()
        ));
    }
    if package.revision() != selection.expected_revision() {
        return Err(format!(
            "resolved package revision `{}` does not match selected revision `{}`",
            package.revision(),
            selection.expected_revision()
        ));
    }
    if package.format_version() == SKILL_PACKAGE_FORMAT_VERSION && !package.resources().is_empty() {
        return Err("package format v1 cannot expose sibling resources".to_string());
    }
    Ok(())
}

fn source_diagnostic(
    source_id: &SkillSourceId,
    code: SkillDiagnosticCode,
    message: impl Into<String>,
) -> SkillDiagnostic {
    SkillDiagnostic::new(
        code,
        SkillDiagnosticSeverity::Error,
        message.into(),
        source_id.as_str().to_string(),
    )
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

fn validate_unique_selections(
    selections: &[SkillSelection],
) -> Result<Vec<(usize, &SkillSelection)>, SkillActivationError> {
    let mut first_by_id = HashMap::with_capacity(selections.len());
    let mut unique = Vec::with_capacity(selections.len());
    for (index, selection) in selections.iter().enumerate() {
        match first_by_id.get(selection.skill_id()) {
            Some(&first_index) => {
                let first: &SkillSelection = &selections[first_index];
                return Err(SkillActivationError::DuplicateSelection {
                    skill_id: selection.skill_id().clone(),
                    first_index,
                    duplicate_index: index,
                    first_revision: first.expected_revision().clone(),
                    duplicate_revision: selection.expected_revision().clone(),
                });
            }
            None => {
                first_by_id.insert(selection.skill_id().clone(), index);
                unique.push((index, selection));
            }
        }
    }
    Ok(unique)
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
mod tests {
    use super::*;
    use crate::skills::digest::package_revision;
    use crate::skills::model::{
        SkillActivationScope, SkillDescriptorParts, SkillErrorCode, SkillProvenance, SkillRecovery,
        SkillRevision, SkillSourceKind, SkillTrust, SKILL_PACKAGE_FORMAT_VERSION,
    };
    use crate::skills::source::WorkspaceSkillSource;
    use crate::skills::workspace::{AGENTS_DIRECTORY, SKILLS_DIRECTORY, SKILL_FILE_NAME};
    use std::fs;
    use std::ops::Range;
    use tempfile::tempdir;

    #[derive(Debug)]
    struct MockSource {
        id: SkillSourceId,
        packages: BTreeMap<super::super::model::SkillId, ResolvedSkillPackage>,
    }

    impl MockSource {
        fn new(authority: &str, entries: &[(&str, &str, &str)]) -> Self {
            let id = SkillSourceId::parse(format!("bundled:{authority}")).unwrap();
            let mut packages = BTreeMap::new();
            for (local_id, name, instructions) in entries {
                let source_text =
                    format!("---\nname: {name}\ndescription: Mock {name}.\n---\n{instructions}");
                let instructions_start = source_text.len() - instructions.len();
                let skill_id =
                    super::super::model::SkillId::from_parts(id.clone(), local_id).unwrap();
                let descriptor = SkillDescriptor::new(SkillDescriptorParts {
                    id: skill_id.clone(),
                    name: (*name).to_string(),
                    description: format!("Mock {name}."),
                    source_kind: SkillSourceKind::Bundled,
                    trust: SkillTrust::Application,
                    activation_scope: SkillActivationScope::Run,
                    revision: package_revision(source_text.as_bytes()),
                    provenance: SkillProvenance::Bundled {
                        source_id: id.clone(),
                        relative_path: format!("{local_id}/SKILL.md"),
                    },
                });
                let range = Range {
                    start: instructions_start,
                    end: source_text.len(),
                };
                packages.insert(
                    skill_id,
                    ResolvedSkillPackage::new(descriptor, Arc::from(source_text), range).unwrap(),
                );
            }
            Self { id, packages }
        }
    }

    impl SkillSource for MockSource {
        fn id(&self) -> &SkillSourceId {
            &self.id
        }

        fn kind(&self) -> SkillSourceKind {
            SkillSourceKind::Bundled
        }

        fn trust(&self) -> super::super::model::SkillTrust {
            super::super::model::SkillTrust::Application
        }

        fn activation_scope(&self) -> SkillActivationScope {
            SkillActivationScope::Run
        }

        fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
            Ok(finalize_catalog(
                self.packages
                    .values()
                    .map(|package| package.descriptor().clone())
                    .collect(),
                Vec::new(),
                false,
            ))
        }

        fn resolve(
            &self,
            selection: &SkillSelection,
        ) -> Result<ResolvedSkillPackage, SkillResolveError> {
            let Some(package) = self.packages.get(selection.skill_id()) else {
                return Err(SkillResolveError::NotFound {
                    skill_id: selection.skill_id().clone(),
                });
            };
            if package.revision() != selection.expected_revision() {
                return Err(SkillResolveError::Stale {
                    skill_id: selection.skill_id().clone(),
                    expected_revision: selection.expected_revision().clone(),
                    actual_revision: package.revision().clone(),
                });
            }
            Ok(package.clone())
        }
    }

    #[derive(Debug)]
    struct FailingSource {
        id: SkillSourceId,
    }

    impl FailingSource {
        fn new(authority: &str) -> Self {
            Self {
                id: SkillSourceId::parse(format!("bundled:{authority}")).unwrap(),
            }
        }
    }

    impl SkillSource for FailingSource {
        fn id(&self) -> &SkillSourceId {
            &self.id
        }

        fn kind(&self) -> SkillSourceKind {
            SkillSourceKind::Bundled
        }

        fn trust(&self) -> SkillTrust {
            SkillTrust::Application
        }

        fn activation_scope(&self) -> SkillActivationScope {
            SkillActivationScope::Run
        }

        fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
            Err(SkillDiscoveryError::InvalidSource {
                reason: "fixture source is offline".to_string(),
            })
        }

        fn resolve(
            &self,
            selection: &SkillSelection,
        ) -> Result<ResolvedSkillPackage, SkillResolveError> {
            Err(SkillResolveError::Unavailable {
                skill_id: Some(selection.skill_id().clone()),
                reason: "fixture source is offline".to_string(),
            })
        }
    }

    #[derive(Debug)]
    struct LyingResolveSource {
        id: SkillSourceId,
        package: ResolvedSkillPackage,
    }

    impl SkillSource for LyingResolveSource {
        fn id(&self) -> &SkillSourceId {
            &self.id
        }

        fn kind(&self) -> SkillSourceKind {
            SkillSourceKind::Bundled
        }

        fn trust(&self) -> SkillTrust {
            SkillTrust::Application
        }

        fn activation_scope(&self) -> SkillActivationScope {
            SkillActivationScope::Run
        }

        fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
            Ok(finalize_catalog(
                vec![self.package.descriptor().clone()],
                Vec::new(),
                false,
            ))
        }

        fn resolve(
            &self,
            _selection: &SkillSelection,
        ) -> Result<ResolvedSkillPackage, SkillResolveError> {
            Ok(self.package.clone())
        }
    }

    fn write_workspace_skill(workspace: &Path, directory: &str) {
        let directory = workspace
            .join(AGENTS_DIRECTORY)
            .join(SKILLS_DIRECTORY)
            .join(directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(SKILL_FILE_NAME),
            "---\nname: workspace-skill\ndescription: Workspace fixture.\n---\n# Run\n",
        )
        .unwrap();
    }

    #[test]
    fn registry_routes_every_descriptor_to_its_owning_source() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let workspace_source =
            Arc::new(WorkspaceSkillSource::new("workspace", workspace.path()).unwrap());
        let bundled_source = Arc::new(MockSource::new(
            "test",
            &[("bundled-skill", "bundled-skill", "# Bundled\n")],
        ));
        let service = SkillsService::from_sources([
            workspace_source as Arc<dyn SkillSource>,
            bundled_source as Arc<dyn SkillSource>,
        ])
        .unwrap();

        let catalog = service.list().unwrap();
        assert_eq!(catalog.skills().len(), 2);
        for descriptor in catalog.skills() {
            let package = service.resolve(&descriptor.selection()).unwrap();
            assert_eq!(package.descriptor(), descriptor);
            assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
            assert!(package.resources().is_empty());
        }
        assert!(catalog
            .skills()
            .iter()
            .any(|skill| skill.source_kind() == SkillSourceKind::Bundled));
        assert!(catalog
            .skills()
            .iter()
            .any(|skill| skill.source_kind() == SkillSourceKind::Workspace));
    }

    #[test]
    fn registry_rejects_a_source_id_that_impersonates_another_kind() {
        let mut source = MockSource::new("kind-mismatch", &[("review", "review", "# Review\n")]);
        source.id = SkillSourceId::parse("workspace:kind-mismatch").unwrap();

        let error =
            SkillsService::from_sources([Arc::new(source) as Arc<dyn SkillSource>]).unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::InvalidSource);
        assert!(matches!(
            error,
            SkillRegistrationError::InvalidSource { reason }
                if reason.contains("declares kind `workspace`")
                    && reason.contains("reports kind `bundled`")
        ));
    }

    #[test]
    fn catalog_rejects_provenance_that_is_not_bound_to_the_skill_id() {
        let source = MockSource::new("provenance", &[("review", "review", "# Review\n")]);
        let original = source.packages.values().next().unwrap();
        let descriptor = SkillDescriptor::new(SkillDescriptorParts {
            id: original.id().clone(),
            name: original.name().to_string(),
            description: original.description().to_string(),
            source_kind: original.source_kind(),
            trust: original.trust(),
            activation_scope: original.activation_scope(),
            revision: original.revision().clone(),
            provenance: SkillProvenance::Bundled {
                source_id: source.id.clone(),
                relative_path: "another-skill/SKILL.md".to_string(),
            },
        });
        let package = ResolvedSkillPackage::new(
            descriptor,
            Arc::from(original.source_text()),
            original.instructions_range().clone(),
        )
        .unwrap();
        let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
            id: source.id,
            package,
        }) as Arc<dyn SkillSource>])
        .unwrap();

        let catalog = service.list().unwrap();

        assert!(catalog.skills().is_empty());
        assert!(catalog.diagnostics().iter().any(|diagnostic| {
            diagnostic.code() == SkillDiagnosticCode::SourceContractViolation
                && diagnostic.message().contains("another-skill/SKILL.md")
        }));
    }

    #[test]
    fn resolved_contract_binds_revision_to_the_exact_source_snapshot() {
        let source = MockSource::new("digest", &[("review", "review", "# Good\n")]);
        let original = source.packages.values().next().unwrap();
        let tampered_source = original.source_text().replace("# Good\n", "# Evil\n");
        let package = ResolvedSkillPackage::new(
            original.descriptor().clone(),
            Arc::from(tampered_source),
            original.instructions_range().clone(),
        )
        .unwrap();
        let selection = package.descriptor().selection();
        let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
            id: source.id,
            package,
        }) as Arc<dyn SkillSource>])
        .unwrap();

        let error = service.resolve(&selection).unwrap_err();

        assert!(matches!(
            error,
            SkillResolveError::SourceContractViolation { reason, .. }
                if reason.contains("does not match its source snapshot")
        ));
    }

    #[test]
    fn resolved_contract_reparses_metadata_and_instruction_boundaries() {
        let source = MockSource::new("parser", &[("review", "review", "# Review\n")]);
        let original = source.packages.values().next().unwrap();
        let mismatched_descriptor = SkillDescriptor::new(SkillDescriptorParts {
            id: original.id().clone(),
            name: "spoofed-name".to_string(),
            description: original.description().to_string(),
            source_kind: original.source_kind(),
            trust: original.trust(),
            activation_scope: original.activation_scope(),
            revision: original.revision().clone(),
            provenance: original.provenance().clone(),
        });
        let metadata_package = ResolvedSkillPackage::new(
            mismatched_descriptor,
            Arc::from(original.source_text()),
            original.instructions_range().clone(),
        )
        .unwrap();
        let metadata_selection = metadata_package.descriptor().selection();
        let metadata_service = SkillsService::from_sources([Arc::new(LyingResolveSource {
            id: source.id.clone(),
            package: metadata_package,
        }) as Arc<dyn SkillSource>])
        .unwrap();

        let metadata_error = metadata_service.resolve(&metadata_selection).unwrap_err();
        assert!(matches!(
            metadata_error,
            SkillResolveError::SourceContractViolation { reason, .. }
                if reason.contains("metadata name")
        ));

        let shifted_range =
            original.instructions_range().start + 1..original.instructions_range().end;
        let range_package = ResolvedSkillPackage::new(
            original.descriptor().clone(),
            Arc::from(original.source_text()),
            shifted_range,
        )
        .unwrap();
        let range_selection = range_package.descriptor().selection();
        let range_service = SkillsService::from_sources([Arc::new(LyingResolveSource {
            id: source.id,
            package: range_package,
        }) as Arc<dyn SkillSource>])
        .unwrap();

        let range_error = range_service.resolve(&range_selection).unwrap_err();
        assert!(matches!(
            range_error,
            SkillResolveError::SourceContractViolation { reason, .. }
                if reason.contains("instruction range")
        ));
    }

    #[test]
    fn resolved_bundled_contract_rejects_a_descriptor_supplied_fallback_name() {
        let source_id = SkillSourceId::parse("bundled:missing-name").unwrap();
        let skill_id =
            super::super::model::SkillId::from_parts(source_id.clone(), "review").unwrap();
        let source_text = "---\ndescription: Missing explicit name.\n---\n# Review\n";
        let instructions_start = source_text.find("# Review").unwrap();
        let descriptor = SkillDescriptor::new(SkillDescriptorParts {
            id: skill_id,
            name: "review".to_string(),
            description: "Missing explicit name.".to_string(),
            source_kind: SkillSourceKind::Bundled,
            trust: SkillTrust::Application,
            activation_scope: SkillActivationScope::Run,
            revision: package_revision(source_text.as_bytes()),
            provenance: SkillProvenance::Bundled {
                source_id: source_id.clone(),
                relative_path: "review/SKILL.md".to_string(),
            },
        });
        let package = ResolvedSkillPackage::new(
            descriptor,
            Arc::from(source_text),
            instructions_start..source_text.len(),
        )
        .unwrap();
        let selection = package.descriptor().selection();
        let service = SkillsService::from_sources([Arc::new(LyingResolveSource {
            id: source_id,
            package,
        }) as Arc<dyn SkillSource>])
        .unwrap();

        let error = service.resolve(&selection).unwrap_err();

        assert!(matches!(
            error,
            SkillResolveError::SourceContractViolation { reason, .. }
                if reason.contains("must declare an explicit name")
        ));
    }

    #[test]
    fn resolved_package_constructor_rejects_invalid_instruction_ranges() {
        let source = MockSource::new("range", &[("review", "review", "# Review\n")]);
        let descriptor = source
            .packages
            .values()
            .next()
            .unwrap()
            .descriptor()
            .clone();

        let reversed_range = Range { start: 3, end: 2 };
        let reversed =
            ResolvedSkillPackage::new(descriptor.clone(), Arc::from("text"), reversed_range)
                .unwrap_err();
        assert!(reversed.to_string().contains("starts after"));

        let outside =
            ResolvedSkillPackage::new(descriptor.clone(), Arc::from("text"), 0..5).unwrap_err();
        assert!(outside.to_string().contains("exceeds"));

        let utf8 = ResolvedSkillPackage::new(descriptor, Arc::from("é"), 1..2).unwrap_err();
        assert!(utf8.to_string().contains("UTF-8"));
    }

    #[test]
    fn explicit_bundled_registration_exposes_the_real_embedded_skill() {
        let empty = SkillsService::new().list().unwrap();
        assert!(empty.skills().is_empty());

        let service = SkillsService::new().with_bundled_source().unwrap();
        let catalog = service.list().unwrap();

        assert_eq!(catalog.skills().len(), 1);
        let descriptor = &catalog.skills()[0];
        assert_eq!(
            descriptor.id().as_str(),
            "bundled:application:repository-evidence-auditor"
        );
        assert_eq!(descriptor.source_kind(), SkillSourceKind::Bundled);
        assert_eq!(descriptor.trust(), SkillTrust::Application);
        assert!(catalog.diagnostics().is_empty());

        let package = service.resolve(&descriptor.selection()).unwrap();
        assert_eq!(package.descriptor(), descriptor);
        assert_eq!(package.format_version(), SKILL_PACKAGE_FORMAT_VERSION);
        assert!(package.resources().is_empty());
    }

    #[test]
    fn list_with_workspace_aggregates_real_sources_in_stable_order() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let service = SkillsService::new().with_bundled_source().unwrap();

        let first = service
            .list_with_workspace("workspace", workspace.path())
            .unwrap();
        let second = service
            .list_with_workspace("workspace", workspace.path())
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.skills().len(), 2);
        assert_eq!(
            first
                .skills()
                .iter()
                .map(|skill| skill.id().as_str())
                .collect::<Vec<_>>(),
            vec![
                "bundled:application:repository-evidence-auditor",
                "workspace:workspace:workspace-skill",
            ]
        );
        assert_eq!(
            first
                .skills()
                .iter()
                .map(SkillDescriptor::source_kind)
                .collect::<Vec<_>>(),
            vec![SkillSourceKind::Bundled, SkillSourceKind::Workspace]
        );
        assert!(first.diagnostics().is_empty());

        // The request-scoped workspace must not mutate the registered source set.
        let registered = service.list().unwrap();
        assert_eq!(registered.skills().len(), 1);
        assert_eq!(
            registered.skills()[0].source_kind(),
            SkillSourceKind::Bundled
        );
    }

    #[test]
    fn list_with_workspace_rejects_a_registered_workspace_collision() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let service = SkillsService::for_workspace("workspace", workspace.path()).unwrap();

        let error = service
            .list_with_workspace("workspace", workspace.path())
            .unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::DuplicateSource);
        assert!(matches!(
            error,
            SkillRegistrationError::DuplicateSource { source_id }
                if source_id.as_str() == "workspace:workspace"
        ));
        assert_eq!(service.list().unwrap().skills().len(), 1);
    }

    #[test]
    fn real_cross_source_activation_preserves_user_selection_order() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let service = SkillsService::new().with_bundled_source().unwrap();
        let catalog = service
            .list_with_workspace("workspace", workspace.path())
            .unwrap();
        let bundled = catalog
            .skills()
            .iter()
            .find(|skill| skill.source_kind() == SkillSourceKind::Bundled)
            .unwrap()
            .selection();
        let workspace_skill = catalog
            .skills()
            .iter()
            .find(|skill| skill.source_kind() == SkillSourceKind::Workspace)
            .unwrap()
            .selection();

        let activated = service
            .activate_workspace("workspace", workspace.path(), &[workspace_skill, bundled])
            .unwrap();

        assert_eq!(
            activated
                .skills()
                .iter()
                .map(ResolvedSkillPackage::source_kind)
                .collect::<Vec<_>>(),
            vec![SkillSourceKind::Workspace, SkillSourceKind::Bundled]
        );
    }

    #[test]
    fn catalog_keeps_healthy_sources_when_another_source_is_unavailable() {
        let healthy = Arc::new(MockSource::new(
            "healthy",
            &[("review", "review", "# Review\n")],
        ));
        let failing = Arc::new(FailingSource::new("offline"));
        let service = SkillsService::from_sources([
            healthy as Arc<dyn SkillSource>,
            failing as Arc<dyn SkillSource>,
        ])
        .unwrap();

        let catalog = service.list().unwrap();

        assert_eq!(catalog.skills().len(), 1);
        assert_eq!(catalog.skills()[0].name(), "review");
        assert!(catalog
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == SkillDiagnosticCode::SourceUnavailable));
    }

    #[test]
    fn registry_rejects_a_source_that_returns_a_different_package() {
        let valid = MockSource::new("lying", &[("returned", "returned", "# Returned\n")]);
        let source_id = valid.id.clone();
        let package = valid.packages.values().next().unwrap().clone();
        let source = Arc::new(LyingResolveSource {
            id: source_id.clone(),
            package,
        });
        let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
        let requested_id =
            super::super::model::SkillId::from_parts(source_id, "requested").unwrap();
        let selection = SkillSelection::new(
            requested_id,
            service.list().unwrap().skills()[0].revision().clone(),
        );

        let error = service.resolve(&selection).unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::ResolveFailed);
        assert_eq!(
            error.recovery(),
            super::super::model::SkillRecovery::ReconfigureSource
        );
        assert!(matches!(
            error,
            SkillResolveError::SourceContractViolation { .. }
        ));
    }

    #[test]
    fn request_scoped_workspace_activation_keeps_other_registered_sources() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let bundled_source = Arc::new(MockSource::new(
            "test",
            &[("bundled-skill", "bundled-skill", "# Bundled\n")],
        ));
        let service =
            SkillsService::from_sources([bundled_source as Arc<dyn SkillSource>]).unwrap();
        let bundled_selection = service.list().unwrap().skills()[0].selection();
        let workspace_selection = service
            .list_workspace("workspace", workspace.path())
            .unwrap()
            .skills()[0]
            .selection();

        let activated = service
            .activate_workspace(
                "workspace",
                workspace.path(),
                &[workspace_selection, bundled_selection],
            )
            .unwrap();

        assert_eq!(
            activated
                .skills()
                .iter()
                .map(ResolvedSkillPackage::source_kind)
                .collect::<Vec<_>>(),
            vec![SkillSourceKind::Workspace, SkillSourceKind::Bundled]
        );
    }

    #[test]
    fn request_scoped_workspace_activation_rejects_an_ambiguous_registered_source() {
        let workspace = tempdir().unwrap();
        write_workspace_skill(workspace.path(), "workspace-skill");
        let service = SkillsService::for_workspace("workspace", workspace.path()).unwrap();
        let selection = service.list().unwrap().skills()[0].selection();

        let error = service
            .activate_workspace("workspace", workspace.path(), &[selection])
            .unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::DuplicateSource);
        assert!(matches!(
            error,
            SkillActivationError::SourceRegistration {
                source: SkillRegistrationError::DuplicateSource { .. }
            }
        ));
    }

    #[test]
    fn source_identity_requires_non_empty_kind_and_authority() {
        assert!(SkillSourceId::parse(":authority").is_err());
        assert!(SkillSourceId::parse("kind:").is_err());
        assert!(SkillSourceId::parse("kind").is_err());
        assert!(SkillSourceId::parse("kind:authority:").is_err());
        assert!(SkillSourceId::parse("kind::authority").is_err());
        assert!(SkillSourceId::parse("kind:authority").is_ok());
    }

    #[test]
    fn workspace_not_directory_requires_source_reconfiguration() {
        let error = SkillResolveError::Workspace(SkillDiscoveryError::WorkspaceNotDirectory {
            path: PathBuf::from("/not-a-workspace"),
        });

        assert_eq!(error.code(), SkillErrorCode::WorkspaceNotDirectory);
        assert_eq!(error.recovery(), SkillRecovery::ReconfigureSource);
        assert!(!error.is_retryable());

        let fixture = tempdir().unwrap();
        let workspace_file = fixture.path().join("workspace-file");
        fs::write(&workspace_file, "not a directory").unwrap();
        let selection = SkillSelection::parse("workspace:workspace:reviewer", "revision").unwrap();
        let activation_error = SkillsService::new()
            .activate_workspace("workspace", &workspace_file, &[selection])
            .unwrap_err();
        assert_eq!(
            activation_error.code(),
            SkillErrorCode::WorkspaceNotDirectory
        );
        assert_eq!(
            activation_error.recovery(),
            SkillRecovery::ReconfigureSource
        );
    }

    #[test]
    fn activation_preserves_selection_order_and_has_a_stable_revision() {
        let source = Arc::new(MockSource::new(
            "test",
            &[
                ("alpha", "alpha", "# Alpha\n"),
                ("beta", "beta", "# Beta\n"),
            ],
        ));
        let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
        let catalog = service.list().unwrap();
        let alpha = catalog
            .skills()
            .iter()
            .find(|skill| skill.name() == "alpha")
            .unwrap()
            .selection();
        let beta = catalog
            .skills()
            .iter()
            .find(|skill| skill.name() == "beta")
            .unwrap()
            .selection();

        let selections = [beta.clone(), alpha.clone()];
        let first = service.activate(&selections).unwrap();
        let second = service.activate(&selections).unwrap();
        let reversed = service.activate(&[alpha, beta]).unwrap();

        assert_eq!(
            first
                .skills()
                .iter()
                .map(|skill| skill.name())
                .collect::<Vec<_>>(),
            vec!["beta", "alpha"]
        );
        assert_eq!(first.revision(), second.revision());
        assert_ne!(first.revision(), reversed.revision());
        assert_eq!(
            first.total_source_bytes(),
            first
                .skills()
                .iter()
                .map(|skill| skill.source_text().len())
                .sum::<usize>()
        );
    }

    #[test]
    fn activation_rejects_duplicate_ids_even_when_revisions_match() {
        let source = Arc::new(MockSource::new("test", &[("alpha", "alpha", "# Alpha\n")]));
        let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
        let selection = service.list().unwrap().skills()[0].selection();

        let error = service
            .activate(&[selection.clone(), selection])
            .unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::DuplicateSelection);
        assert_eq!(error.selection_index(), Some(1));
        assert_eq!(
            error.recovery(),
            super::super::model::SkillRecovery::ChangeSelection
        );
    }

    #[test]
    fn activation_is_all_or_nothing_and_reports_the_original_index() {
        let source = Arc::new(MockSource::new(
            "test",
            &[
                ("alpha", "alpha", "# Alpha\n"),
                ("beta", "beta", "# Beta\n"),
            ],
        ));
        let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
        let catalog = service.list().unwrap();
        let good = catalog.skills()[0].selection();
        let stale = SkillSelection::new(
            catalog.skills()[1].id().clone(),
            SkillRevision::parse("stale-revision").unwrap(),
        );

        let error = service.activate(&[good, stale]).unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::Stale);
        assert_eq!(error.selection_index(), Some(1));
        assert!(error.actual_revision().is_some());
        assert!(error
            .source_error()
            .is_some_and(SkillResolveError::is_stale));
    }

    #[test]
    fn activation_enforces_count_and_aggregate_source_byte_budgets() {
        let source = Arc::new(MockSource::new(
            "test",
            &[
                ("alpha", "alpha", "# Alpha\n"),
                ("beta", "beta", "# Beta\n"),
            ],
        ));
        let service = SkillsService::from_sources([source as Arc<dyn SkillSource>]).unwrap();
        let selections = service
            .list()
            .unwrap()
            .skills()
            .iter()
            .map(SkillDescriptor::selection)
            .collect::<Vec<_>>();

        let count_limited = service
            .clone_for_test(SkillActivationPolicy::new(1, usize::MAX))
            .activate(&selections)
            .unwrap_err();
        assert_eq!(count_limited.code(), SkillErrorCode::TooManySkills);

        let byte_limited = service
            .clone_for_test(SkillActivationPolicy::new(2, 1))
            .activate(&selections)
            .unwrap_err();
        assert_eq!(byte_limited.code(), SkillErrorCode::SourceBudgetExceeded);
        assert_eq!(
            byte_limited.recovery(),
            super::super::model::SkillRecovery::ReduceSelection
        );
    }

    #[test]
    fn oversized_workspace_identity_is_rejected_before_catalog_generation() {
        let workspace = tempdir().unwrap();
        let workspace_id = "w".repeat(16 * 1024);

        let error = SkillsService::new()
            .list_workspace(&workspace_id, workspace.path())
            .unwrap_err();

        assert_eq!(error.code(), SkillErrorCode::InvalidSource);
        assert_eq!(
            error.recovery(),
            super::super::model::SkillRecovery::ReconfigureSource
        );
    }

    impl SkillsService {
        fn clone_for_test(&self, policy: SkillActivationPolicy) -> Self {
            Self {
                registry: self.registry.clone(),
                activation_policy: policy,
            }
        }
    }
}
