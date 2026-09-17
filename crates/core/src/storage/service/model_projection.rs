use super::agent_templates::resolve_exact_agent_model;
use super::settings::model_settings_catalog_snapshot;
use super::*;
use crate::storage::models::{
    CredentialStatus, ModelConfigRecord, ModelExecutionStatus, ModelSettingsSnapshot,
    StoredModelConfigRecord, StoredModelSettingsSnapshot,
};
use crate::AgentModelUnavailableReason;

/// The Host-authoritative availability projection for every configured model.
///
/// One settings snapshot under the model credential coordinator lock produces this projection;
/// every consumer that offers, validates, or advertises a model — selectors, allow-lists,
/// settings editors, automation targets, and templates — must derive its answer from it instead
/// of re-implementing the availability rules.
#[derive(Debug, Clone)]
pub struct ModelProjection {
    /// Opaque compare-and-swap identity of the settings snapshot this projection came from.
    pub configuration_revision: String,
    /// One entry per stored model configuration, in stored order.
    pub models: Vec<ModelProjectionEntry>,
}

/// One configured model together with its current execution status.
#[derive(Debug, Clone)]
pub struct ModelProjectionEntry {
    /// Credential-free projection of the stored model configuration.
    pub model: ModelConfigRecord,
    /// The single execution status judgement for this model.
    pub execution: ModelExecutionStatus,
}

impl ModelProjection {
    /// Finds the projection entry for one model configuration identity.
    pub fn entry(&self, model_config_id: &str) -> Option<&ModelProjectionEntry> {
        self.models
            .iter()
            .find(|entry| entry.model.id == model_config_id)
    }
}

impl StorageService {
    /// Loads the single availability projection shared by every model consumer.
    ///
    /// A model is `Available` only when it is enabled, its spawn-viability identity resolves
    /// (connection, profile, revisions, runtime), and its effective credential resolves in the
    /// active credential backend. Credential failures map to `CredentialMissing` /
    /// `CredentialUnavailable`; every other reason comes from `resolve_exact_agent_model`.
    pub fn load_model_projection(&self) -> Result<Option<ModelProjection>, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };
        Ok(stored
            .as_ref()
            .map(|snapshot| self.model_projection_from_stored(snapshot)))
    }

    /// Computes the projection from an already-read stored snapshot.
    ///
    /// Callers must hold the model credential coordinator lock so the settings snapshot and the
    /// credential session stay consistent; this projection module never re-locks it.
    pub(super) fn model_projection_from_stored(
        &self,
        stored: &StoredModelSettingsSnapshot,
    ) -> ModelProjection {
        let catalog = model_settings_catalog_snapshot(stored);
        debug_assert_eq!(
            catalog.settings.models.len(),
            stored.settings.models.len(),
            "the catalog snapshot maps stored models one-to-one"
        );
        let models = stored
            .settings
            .models
            .iter()
            .enumerate()
            .map(|(index, stored_model)| ModelProjectionEntry {
                model: catalog.settings.models[index].clone(),
                execution: self.model_execution_status(stored, stored_model, &catalog),
            })
            .collect();
        ModelProjection {
            configuration_revision: stored.configuration_revision.clone(),
            models,
        }
    }

    fn model_execution_status(
        &self,
        stored: &StoredModelSettingsSnapshot,
        stored_model: &StoredModelConfigRecord,
        catalog: &ModelSettingsSnapshot,
    ) -> ModelExecutionStatus {
        if let Err(reason) = resolve_exact_agent_model(catalog, &stored_model.id) {
            return ModelExecutionStatus::Unavailable { reason };
        }
        match self.effective_connection_credential_status(&stored.settings, stored_model) {
            CredentialStatus::Configured => ModelExecutionStatus::Available,
            // A structurally valid model normally carries a reference for its effective
            // connection, so this arm is the typed mapping for any classifier state that reports
            // an absent credential while the identity checks still pass.
            CredentialStatus::Missing => ModelExecutionStatus::Unavailable {
                reason: AgentModelUnavailableReason::CredentialMissing,
            },
            CredentialStatus::Unavailable => ModelExecutionStatus::Unavailable {
                reason: AgentModelUnavailableReason::CredentialUnavailable,
            },
        }
    }
}
