use super::discovery::list_workspace_source;
use super::model::{
    ResolvedSkillPackage, SkillActivationScope, SkillCatalog, SkillDiscoveryError,
    SkillReferenceError, SkillRegistrationError, SkillResolveError, SkillSelection, SkillSourceId,
    SkillSourceKind, SkillTrust,
};
use super::resolver::resolve_workspace_source;
use super::resource_runtime::{
    restore_resolve_error, SkillResourceError, SkillResourceReaderRef, SkillResourceSessionBinding,
};
use super::workspace::percent_encode;
use std::path::{Path, PathBuf};

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
}

fn validate_workspace_id(workspace_id: &str) -> Result<(), SkillReferenceError> {
    if workspace_id.is_empty() {
        return Err(SkillReferenceError::new("workspace id must not be empty"));
    }
    Ok(())
}
