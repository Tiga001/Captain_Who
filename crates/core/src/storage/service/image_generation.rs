use super::*;
use crate::storage::image_generation_execution_repository::{
    self, ImageGenerationArtifactJournalRecord, ImageGenerationExecutionClaimOutcome,
    ImageGenerationExecutionIdentityRecord, ImageGenerationExecutionJournalRecord,
    ImageGenerationExecutionMutationOutcome, ImageGenerationExecutionTerminalUpdate,
};

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
}
