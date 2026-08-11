use super::*;
use crate::storage::image_generation_execution_repository::{
    self, ImageGenerationArtifactJournalRecord, ImageGenerationExecutionClaimOutcome,
    ImageGenerationExecutionIdentityRecord, ImageGenerationExecutionJournalRecord,
    ImageGenerationExecutionMutationOutcome, ImageGenerationExecutionTerminalUpdate,
    PublishedImageArtifactInputRecord,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGeneratedArtifactInput {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub kind: ResolvedGeneratedArtifactKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedGeneratedArtifactKind {
    ImageGeneration,
    Managed(crate::storage::managed_artifact_repository::ManagedArtifactKind),
}

impl ResolvedGeneratedArtifactKind {
    pub const fn is_image(self) -> bool {
        matches!(
            self,
            Self::ImageGeneration
                | Self::Managed(
                    crate::storage::managed_artifact_repository::ManagedArtifactKind::Image
                )
        )
    }
}

impl StorageService {
    pub fn claim_image_generation_execution(
        &self,
        identity: &ImageGenerationExecutionIdentityRecord,
    ) -> Result<ImageGenerationExecutionClaimOutcome, String> {
        let mut connection = self.state.connection()?;
        image_generation_execution_repository::claim_image_generation_execution(
            &mut connection,
            identity,
        )
        .map_err(storage_error)
    }

    pub fn prepare_image_generation_artifact(
        &self,
        execution_id: &str,
        artifact: &ImageGenerationArtifactJournalRecord,
        provider_request_id: Option<&str>,
        http_status: Option<u16>,
    ) -> Result<ImageGenerationExecutionMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        image_generation_execution_repository::prepare_image_generation_artifact(
            &mut connection,
            execution_id,
            artifact,
            provider_request_id,
            http_status,
        )
        .map_err(storage_error)
    }

    pub fn finalize_image_generation_execution(
        &self,
        execution_id: &str,
        update: &ImageGenerationExecutionTerminalUpdate,
    ) -> Result<ImageGenerationExecutionMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        image_generation_execution_repository::finalize_image_generation_execution(
            &mut connection,
            execution_id,
            update,
        )
        .map_err(storage_error)
    }

    pub fn inspect_image_generation_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<ImageGenerationExecutionJournalRecord>, String> {
        let connection = self.state.connection()?;
        image_generation_execution_repository::inspect_image_generation_execution(
            &connection,
            execution_id,
        )
        .map_err(storage_error)
    }

    pub fn list_interrupted_image_generation_executions(
        &self,
    ) -> Result<Vec<ImageGenerationExecutionJournalRecord>, String> {
        let connection = self.state.connection()?;
        image_generation_execution_repository::list_interrupted_image_generation_executions(
            &connection,
        )
        .map_err(storage_error)
    }

    /// Resolves one immutable image Artifact from an authoritative publication journal.
    ///
    /// A conversation-authorized managed image takes precedence over the legacy image-generation
    /// journal. Managed documents are deliberately ignored: callers of this method are resolving
    /// an `image-artifact://` capability and must never reinterpret a document grant as an image.
    /// Caller-provided paths and hashes are never consulted here. Duplicate successful legacy
    /// rows are accepted only when every immutable storage identity is identical.
    pub fn resolve_published_generated_artifact_input(
        &self,
        artifact_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<Option<ResolvedGeneratedArtifactInput>, String> {
        if let Some(artifact) =
            self.resolve_published_managed_artifact_input(artifact_id, conversation_id)?
        {
            if artifact.kind.is_image() {
                return Ok(Some(artifact));
            }
        }
        let connection = self.state.connection()?;
        let records = image_generation_execution_repository::list_published_artifact_inputs_by_id(
            &connection,
            artifact_id,
        )
        .map_err(storage_error)?;
        let Some(first) = records.first() else {
            return Ok(None);
        };
        if records
            .iter()
            .any(|record| !same_published_artifact_identity(first, record))
        {
            return Err(
                "generated Artifact publication journal contains conflicting identities"
                    .to_string(),
            );
        }
        let relative = safe_artifact_relative_path(&first.storage_relative_path)?;
        Ok(Some(ResolvedGeneratedArtifactInput {
            path: self.image_artifact_root.join(relative),
            size_bytes: first.size_bytes,
            sha256: first.sha256.clone(),
            kind: ResolvedGeneratedArtifactKind::ImageGeneration,
        }))
    }

    /// Resolves one conversation-authorized managed document for an `artifact://` capability.
    ///
    /// This path intentionally has no legacy image-generation fallback. The content hash alone is
    /// not an authority boundary, and an image publication with the same digest must not authorize
    /// document access in another conversation or under another URI scheme.
    pub fn resolve_published_document_artifact_input(
        &self,
        artifact_id: &str,
        conversation_id: Option<&str>,
    ) -> Result<Option<ResolvedGeneratedArtifactInput>, String> {
        let Some(artifact) =
            self.resolve_published_managed_artifact_input(artifact_id, conversation_id)?
        else {
            return Ok(None);
        };
        if matches!(
            artifact.kind,
            ResolvedGeneratedArtifactKind::Managed(
                crate::storage::managed_artifact_repository::ManagedArtifactKind::Document
            )
        ) {
            Ok(Some(artifact))
        } else {
            Ok(None)
        }
    }
}

fn same_published_artifact_identity(
    left: &PublishedImageArtifactInputRecord,
    right: &PublishedImageArtifactInputRecord,
) -> bool {
    left.artifact_id == right.artifact_id
        && left.storage_relative_path == right.storage_relative_path
        && left.size_bytes == right.size_bytes
        && left.sha256 == right.sha256
}

fn safe_artifact_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.trim().is_empty() || path.is_absolute() {
        return Err("generated Artifact journal contains an invalid storage path".to_string());
    }
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("generated Artifact journal contains an unsafe storage path".to_string())
            }
        }
    }
    if relative.as_os_str().is_empty() {
        Err("generated Artifact journal contains an empty storage path".to_string())
    } else {
        Ok(relative)
    }
}
