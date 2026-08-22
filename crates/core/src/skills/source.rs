use super::discovery::list_workspace_source;
use super::model::{
    ResolvedSkillPackage, SkillActivationScope, SkillCatalog, SkillDiscoveryError,
    SkillReferenceError, SkillRegistrationError, SkillResolveError, SkillSelection, SkillSourceId,
    SkillSourceKind, SkillTrust,
};
use super::resolver::{prepare_workspace_source_snapshot, resolve_workspace_source};
use super::resource_runtime::{
    restore_resolve_error, SkillResourceError, SkillResourceReader, SkillResourceReaderRef,
    SkillResourceSessionBinding, SkillResourceSourceError,
};
use super::workspace::percent_encode;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) trait SkillSource: Send + Sync {
    fn id(&self) -> &SkillSourceId;

    fn kind(&self) -> SkillSourceKind;

    fn trust(&self) -> SkillTrust;

    fn activation_scope(&self) -> SkillActivationScope;

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError>;

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError>;

    /// Open a reader for the exact package snapshot already authorized by
    /// activation. Implementations must not silently follow mutable source
    /// state such as an installation receipt.
    fn open_resource_reader(
        &self,
        package: &ResolvedSkillPackage,
    ) -> Result<Option<SkillResourceReaderRef>, SkillResourceError> {
        if package.resources().is_empty() {
            Ok(None)
        } else {
            Err(SkillResourceError::SourceContractViolation {
                source_id: self.id().clone(),
                reason: format!(
                    "source exposes resources for Skill `{}` without a resource reader",
                    package.id()
                ),
            })
        }
    }

    /// Reconstruct a resource binding from persisted id + revision metadata.
    /// Sources with durable immutable package storage should override this to
    /// bypass mutable catalog pointers.
    fn restore_resource_binding(
        &self,
        selection: &SkillSelection,
    ) -> Result<SkillResourceSessionBinding, SkillResourceError> {
        let package = self.resolve(selection).map_err(|error| {
            restore_resolve_error(selection.skill_id(), selection.expected_revision(), error)
        })?;
        let reader = self.open_resource_reader(&package)?;
        Ok(SkillResourceSessionBinding {
            skill_id: package.id().clone(),
            revision: package.revision().clone(),
            source_id: self.id().clone(),
            resources: package.resources().clone(),
            reader,
        })
    }
}

#[derive(Debug)]
pub(super) struct WorkspaceSkillSource {
    source_id: SkillSourceId,
    workspace_id: String,
    workspace_root: PathBuf,
}

impl WorkspaceSkillSource {
    pub fn new(
        workspace_id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self, SkillRegistrationError> {
        let workspace_id = workspace_id.into();
        validate_workspace_id(&workspace_id).map_err(|error| {
            SkillRegistrationError::InvalidSource {
                reason: error.to_string(),
            }
        })?;
        let source_id = SkillSourceId::parse(format!(
            "workspace:{}",
            percent_encode(workspace_id.as_bytes())
        ))
        .map_err(|error| SkillRegistrationError::InvalidSource {
            reason: error.to_string(),
        })?;
        Ok(Self {
            source_id,
            workspace_id,
            workspace_root: workspace_root.into(),
        })
    }

    pub fn source_id(&self) -> &SkillSourceId {
        &self.source_id
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }
}

impl SkillSource for WorkspaceSkillSource {
    fn id(&self) -> &SkillSourceId {
        self.source_id()
    }

    fn kind(&self) -> SkillSourceKind {
        SkillSourceKind::Workspace
    }

    fn trust(&self) -> SkillTrust {
        SkillTrust::Untrusted
    }

    fn activation_scope(&self) -> SkillActivationScope {
        SkillActivationScope::Run
    }

    fn list(&self) -> Result<SkillCatalog, SkillDiscoveryError> {
        list_workspace_source(self)
    }

    fn resolve(
        &self,
        selection: &SkillSelection,
    ) -> Result<ResolvedSkillPackage, SkillResolveError> {
        resolve_workspace_source(self, selection)
    }

    fn open_resource_reader(
        &self,
        package: &ResolvedSkillPackage,
    ) -> Result<Option<SkillResourceReaderRef>, SkillResourceError> {
        if package.id().source_id() != &self.source_id {
            return Err(SkillResourceError::SourceContractViolation {
                source_id: self.source_id.clone(),
                reason: format!(
                    "Skill `{}` does not belong to this workspace source",
                    package.id()
                ),
            });
        }
        if package.resources().is_empty() {
            return Ok(None);
        }
        let snapshot =
            prepare_workspace_source_snapshot(self, &package.descriptor().selection())
                .map_err(|error| restore_resolve_error(package.id(), package.revision(), error))?;
        if snapshot.package.format_version() != package.format_version()
            || snapshot.package.resource_index() != *package.resources()
        {
            return Err(SkillResourceError::SnapshotIntegrityMismatch {
                skill_id: package.id().clone(),
                revision: package.revision().clone(),
                reason: "workspace package resources changed while activation was prepared"
                    .to_string(),
            });
        }
        Ok(Some(Arc::new(WorkspaceSkillResourceReader {
            package: snapshot.package,
        })))
    }
}

struct WorkspaceSkillResourceReader {
    package: super::prepared::PreparedSkillPackage,
}

impl SkillResourceReader for WorkspaceSkillResourceReader {
    fn read(
        &self,
        expected: &super::model::SkillResourceDescriptor,
    ) -> Result<Vec<u8>, SkillResourceSourceError> {
        let resource = self
            .package
            .resources()
            .iter()
            .find(|resource| resource.descriptor().path() == expected.path())
            .ok_or_else(|| {
                SkillResourceSourceError::Unavailable(
                    "the frozen workspace package has no such resource".to_string(),
                )
            })?;
        if resource.descriptor() != expected {
            return Err(SkillResourceSourceError::Integrity(
                "the frozen workspace resource identity does not match its descriptor".to_string(),
            ));
        }
        Ok(resource.bytes().to_vec())
    }
}

fn validate_workspace_id(workspace_id: &str) -> Result<(), SkillReferenceError> {
    if workspace_id.is_empty() {
        return Err(SkillReferenceError::new("workspace id must not be empty"));
    }
    Ok(())
}
