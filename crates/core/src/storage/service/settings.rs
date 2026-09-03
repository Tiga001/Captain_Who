use super::*;
use crate::image_generation::credential_store::{
    CredentialDeleteOutcome, CredentialReference, CredentialSecret,
};
use crate::storage::models::{
    CredentialMutation, CredentialStatus, ModelConfigEditorRecord, ModelSettingsEditorRecord,
    StoredModelConfigRecord, StoredModelSettingsRecord, StoredModelSettingsSnapshot,
};
use std::collections::BTreeSet;

pub(super) const MAX_SKILL_ENABLEMENT_ID_BYTES: usize = 16 * 1024;
const MAX_PROVIDER_CREDENTIAL_BYTES: usize = 8_192;

struct PlannedCredentialReplacement {
    reference: CredentialReference,
    secret: CredentialSecret,
}

struct PlannedCredentialField {
    credential_ref: Option<String>,
    /// A short-lived value used only by the existing connection validator. For `keep`, this is a
    /// non-secret presence marker; it is never persisted or sent to a provider.
    validation_value: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelProviderCredentialReconciliationReport {
    pub completed_staging: usize,
    pub removed_orphaned_credentials: usize,
    pub removed_retired_credentials: usize,
}

fn validate_provider_credential(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_CREDENTIAL_BYTES
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("provider credential is invalid".to_string());
    }
    Ok(())
}

fn stored_model_from_resolved(
    model: &ModelConfigRecord,
    api_token_override_ref: Option<String>,
) -> StoredModelConfigRecord {
    StoredModelConfigRecord {
        id: model.id.clone(),
        provider_model_id: model.provider_model_id.clone(),
        display_name: model.display_name.clone(),
        api_url_override: model.api_url_override.clone(),
        api_token_override_ref,
        supports_image: model.supports_image,
        context_window_tokens: model.context_window_tokens,
        provider_profile_config: model.provider_profile_config.clone(),
        input_price: model.input_price.clone(),
        cached_input_price: model.cached_input_price.clone(),
        output_price: model.output_price.clone(),
        enabled: model.enabled,
    }
}

fn model_credential_refs(settings: &StoredModelSettingsRecord) -> BTreeSet<String> {
    settings
        .api_token_ref
        .iter()
        .chain(settings.tavily_api_key_ref.iter())
        .chain(
            settings
                .models
                .iter()
                .filter_map(|model| model.api_token_override_ref.as_ref()),
        )
        .cloned()
        .collect()
}

fn model_settings_catalog_snapshot(stored: &StoredModelSettingsSnapshot) -> ModelSettingsSnapshot {
    let presence = |configured: bool| configured.then(|| "configured-credential".to_string());
    ModelSettingsSnapshot {
        settings: ModelSettingsRecord {
            api_url: stored.settings.api_url.clone(),
            api_token: presence(stored.settings.api_token_ref.is_some()).unwrap_or_default(),
            search_mode: stored.settings.search_mode.clone(),
            tavily_api_key: presence(stored.settings.tavily_api_key_ref.is_some())
                .unwrap_or_default(),
            models: stored
                .settings
                .models
                .iter()
                .map(|model| ModelConfigRecord {
                    id: model.id.clone(),
                    provider_model_id: model.provider_model_id.clone(),
                    display_name: model.display_name.clone(),
                    api_url_override: model.api_url_override.clone(),
                    api_token_override: presence(model.api_token_override_ref.is_some()),
                    supports_image: model.supports_image,
                    context_window_tokens: model.context_window_tokens,
                    provider_profile_config: model.provider_profile_config.clone(),
                    input_price: model.input_price.clone(),
                    cached_input_price: model.cached_input_price.clone(),
                    output_price: model.output_price.clone(),
                    enabled: model.enabled,
                })
                .collect(),
        },
        configuration_revision: stored.configuration_revision.clone(),
        provider_connection_revisions: stored.provider_connection_revisions.clone(),
        provider_protocol_revisions: stored.provider_protocol_revisions.clone(),
        search_connection_revision: stored.search_connection_revision.clone(),
    }
}

pub(super) fn validate_model_settings(settings: &ModelSettingsRecord) -> Result<(), String> {
    validate_model_settings_fields(settings).map_err(|error| error.to_string())?;
    for model in &settings.models {
        let model_id = model.provider_model_id.trim();
        model
            .provider_profile_config
            .validate()
            .map_err(|error| format!("模型 {model_id} 的 Provider Profile 无效：{error}"))?;
    }
    Ok(())
}

fn validate_model_settings_fields(
    settings: &ModelSettingsRecord,
) -> Result<(), ModelSettingsSaveError> {
    let global_api_url = settings.api_url.trim();
    if !global_api_url.is_empty() {
        crate::storage::models::validated_connection(
            "全局模型配置",
            "全局",
            global_api_url,
            "configured-credential",
        )?;
    }
    let mut config_ids = HashSet::new();
    let mut normalized_display_names = HashSet::new();
    for model in &settings.models {
        let config_id = model.id.trim();
        if config_id.is_empty() {
            return Err("模型配置 ID 不能为空。".to_string().into());
        }
        if !config_ids.insert(model.id.as_str()) {
            return Err("模型配置 ID 重复。".to_string().into());
        }
        let display_name = model.display_name.trim();
        if display_name.len() > crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES {
            return Err(format!(
                "模型显示名称不能超过 {} 字节。",
                crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES
            )
            .into());
        }
        let normalized_display_name =
            crate::storage::models::normalize_model_display_name(display_name);
        if normalized_display_name.is_empty() {
            return Err("模型显示名称不能为空。".to_string().into());
        }
        if !normalized_display_names.insert(normalized_display_name) {
            return Err(ModelSettingsSaveError::DuplicateDisplayName {
                display_name: display_name.to_string(),
            });
        }
        let provider_model_id = model.provider_model_id.trim();
        if provider_model_id.is_empty() {
            return Err(format!("模型 {display_name} 的厂商模型 ID 不能为空。").into());
        }
        if model.context_window_tokens == Some(0) {
            return Err(format!("模型 {display_name} 的上下文窗口必须大于 0。").into());
        }
        if let Some(api_url_override) = model
            .api_url_override
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            crate::storage::models::validated_connection(
                display_name,
                "专用",
                api_url_override,
                "configured-credential",
            )?;
        }
        if !usage_repository::is_valid_price_per_1k(&model.input_price) {
            return Err(
                format!("模型 {display_name} 的输入价格必须是大于或等于 0 的有效数字。").into(),
            );
        }
        if !model.cached_input_price.trim().is_empty()
            && !usage_repository::is_valid_price_per_1k(&model.cached_input_price)
        {
            return Err(format!(
                "模型 {display_name} 的缓存命中输入价格必须留空，或填写大于或等于 0 的有效数字。"
            )
            .into());
        }
        if !usage_repository::is_valid_price_per_1k(&model.output_price) {
            return Err(
                format!("模型 {display_name} 的输出价格必须是大于或等于 0 的有效数字。").into(),
            );
        }
    }
    Ok(())
}

pub(super) fn validate_skill_enablement_id(skill_id: &str) -> Result<(), String> {
    if skill_id.is_empty() {
        return Err("Skill ID 不能为空。".to_string());
    }
    if skill_id.len() > MAX_SKILL_ENABLEMENT_ID_BYTES {
        return Err(format!(
            "Skill ID 不能超过 {MAX_SKILL_ENABLEMENT_ID_BYTES} 字节。"
        ));
    }
    Ok(())
}

impl StorageService {
    pub fn load_image_generation_profile(
        &self,
        profile_id: &str,
    ) -> Result<Option<ImageGenerationProfileRecord>, String> {
        let connection = self.state.connection()?;
        image_generation_repository::load_image_generation_profile(&connection, profile_id)
            .map_err(storage_error)
    }

    pub fn compare_and_set_image_generation_profile(
        &self,
        profile_id: &str,
        expected_generation: u64,
        replacement: &ImageGenerationProfileRecord,
    ) -> Result<image_generation_repository::ImageGenerationProfileCompareAndSetOutcome, String>
    {
        let mut connection = self.state.connection()?;
        image_generation_repository::compare_and_set_image_generation_profile(
            &mut connection,
            profile_id,
            expected_generation,
            replacement,
        )
        .map_err(storage_error)
    }

    pub fn stage_image_generation_credential(
        &self,
        profile_id: &str,
        expected_generation: u64,
        credential_ref: &str,
    ) -> Result<image_generation_repository::ImageGenerationCredentialStageOutcome, String> {
        let mut connection = self.state.connection()?;
        image_generation_repository::stage_image_generation_credential(
            &mut connection,
            profile_id,
            expected_generation,
            credential_ref,
        )
        .map_err(storage_error)
    }

    pub fn complete_image_generation_credential_staging(
        &self,
        credential_ref: &str,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        image_generation_repository::complete_image_generation_credential_staging(
            &connection,
            credential_ref,
        )
        .map_err(storage_error)
    }

    pub fn list_image_generation_credential_staging(
        &self,
    ) -> Result<Vec<image_generation_repository::ImageGenerationCredentialStagingRecord>, String>
    {
        let connection = self.state.connection()?;
        image_generation_repository::list_image_generation_credential_staging(&connection)
            .map_err(storage_error)
    }

    pub fn list_image_generation_credential_cleanup(
        &self,
    ) -> Result<Vec<image_generation_repository::ImageGenerationCredentialCleanupRecord>, String>
    {
        let connection = self.state.connection()?;
        image_generation_repository::list_image_generation_credential_cleanup(&connection)
            .map_err(storage_error)
    }

    pub fn complete_image_generation_credential_cleanup(
        &self,
        credential_ref: &str,
    ) -> Result<bool, String> {
        let connection = self.state.connection()?;
        image_generation_repository::complete_image_generation_credential_cleanup(
            &connection,
            credential_ref,
        )
        .map_err(storage_error)
    }

    pub fn load_model_settings(&self) -> Result<Option<ModelSettingsRecord>, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings(&mut connection).map_err(storage_error)?
        };
        stored
            .as_ref()
            .map(|settings| self.resolve_model_settings(settings))
            .transpose()
    }

    pub fn load_model_settings_snapshot(&self) -> Result<Option<ModelSettingsSnapshot>, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };
        stored
            .as_ref()
            .map(|snapshot| self.resolve_model_settings_snapshot(snapshot))
            .transpose()
    }

    /// Resolves the minimum secret set required to execute one exact model configuration.
    ///
    /// The returned snapshot deliberately contains only the selected model. An unavailable key
    /// belonging to another catalog entry therefore cannot block this run, and search credentials
    /// are opened only for a search-enabled execution path.
    pub fn load_model_settings_snapshot_for_model(
        &self,
        model_config_id: &str,
        include_search: bool,
    ) -> Result<Option<ModelSettingsSnapshot>, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };
        stored
            .as_ref()
            .map(|snapshot| {
                self.resolve_model_settings_snapshot_for_model(
                    snapshot,
                    model_config_id,
                    include_search,
                )
            })
            .transpose()
    }

    /// Loads the credential-free settings view used by Renderer. Existing secrets and their
    /// opaque references never cross this boundary.
    pub fn load_model_settings_for_edit(
        &self,
    ) -> Result<Option<ModelSettingsEditorRecord>, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };
        stored
            .as_ref()
            .map(|snapshot| self.model_settings_editor_record(snapshot))
            .transpose()
    }

    /// Loads provider metadata and credential presence without opening the native credential
    /// store. Use this for selectors, usage labels and other non-execution surfaces.
    pub fn load_model_settings_catalog(&self) -> Result<Option<ModelSettingsRecord>, String> {
        let stored = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };
        Ok(stored
            .as_ref()
            .map(model_settings_catalog_snapshot)
            .map(|snapshot| snapshot.settings))
    }

    pub fn save_model_settings(&self, settings: ModelSettingsRecord) -> Result<(), String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        validate_model_settings(&settings)?;
        let existing = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings(&mut connection).map_err(storage_error)?
        };
        let (stored, replacements) =
            self.plan_resolved_model_settings(&settings, existing.as_ref())?;
        self.commit_model_settings_plan(stored, replacements, existing.as_ref())
            .map(|_| ())
    }

    /// Applies the Renderer mutation protocol and returns the exact normalized record committed by
    /// the Host. Profile versions and runtime policies are resolved exclusively from the Registry.
    pub fn save_model_settings_request(
        &self,
        request: ModelSettingsSaveRequest,
    ) -> Result<ModelSettingsEditorRecord, ModelSettingsSaveError> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let existing_snapshot = {
            let mut connection = self.state.connection()?;
            config_repository::load_model_settings_snapshot(&mut connection)
                .map_err(storage_error)?
        };

        let ModelSettingsSaveRequest {
            expected_revision,
            api_url,
            api_token_mutation,
            search_mode,
            tavily_api_key_mutation,
            models: requested_models,
        } = request;
        let current_revision = existing_snapshot
            .as_ref()
            .map(|snapshot| snapshot.configuration_revision.as_str());
        let revision_matches = match (expected_revision.as_deref(), current_revision) {
            (None, None) => true,
            (Some(expected), Some(current)) => {
                config_repository::is_model_settings_revision(expected) && expected == current
            }
            _ => false,
        };
        if !revision_matches {
            return Err("model settings revision conflict".to_string().into());
        }
        let existing = existing_snapshot
            .as_ref()
            .map(|snapshot| &snapshot.settings);
        let mut replacements = Vec::new();
        let planned_api_token = self.plan_credential_mutation(
            api_token_mutation,
            existing.and_then(|settings| settings.api_token_ref.as_deref()),
            &mut replacements,
        )?;
        let planned_tavily_api_key = self.plan_credential_mutation(
            tavily_api_key_mutation,
            existing.and_then(|settings| settings.tavily_api_key_ref.as_deref()),
            &mut replacements,
        )?;
        let mut requested_display_names = HashSet::new();
        for requested in &requested_models {
            let display_name = requested.display_name.trim();
            if display_name.len() > crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES {
                return Err(format!(
                    "模型显示名称不能超过 {} 字节。",
                    crate::storage::models::MODEL_DISPLAY_NAME_MAX_BYTES
                )
                .into());
            }
            let normalized = crate::storage::models::normalize_model_display_name(display_name);
            if normalized.is_empty() {
                return Err("模型显示名称不能为空。".to_string().into());
            }
            if !requested_display_names.insert(normalized) {
                return Err(ModelSettingsSaveError::DuplicateDisplayName {
                    display_name: display_name.to_string(),
                });
            }
        }
        let mut models = Vec::with_capacity(requested_models.len());
        let mut stored_models = Vec::with_capacity(requested_models.len());
        let mut matched_existing_ids = HashSet::new();

        for mut requested in requested_models {
            let requested_config_id = requested.id.as_deref();
            let existing_model = match requested_config_id {
                Some(config_id) => {
                    if config_id.trim().is_empty() || config_id != config_id.trim() {
                        return Err("模型配置 ID 无效。".to_string().into());
                    }
                    let model = existing
                        .and_then(|settings| {
                            settings
                                .models
                                .iter()
                                .find(|candidate| candidate.id == config_id)
                        })
                        .ok_or_else(|| format!("未找到要编辑的模型配置：{config_id}"))?;
                    if !matched_existing_ids.insert(config_id.to_string()) {
                        return Err(format!("模型配置 ID 被重复引用：{config_id}").into());
                    }
                    Some(model)
                }
                None => None,
            };
            let config_id = requested
                .id
                .clone()
                .unwrap_or_else(|| format!("model-config:{}", Uuid::new_v4()));
            requested.display_name = requested.display_name.trim().to_string();
            requested.provider_model_id = requested.provider_model_id.trim().to_string();
            let preserved = existing_model.map(|model| model.provider_profile_config.clone());
            let effective_api_url = requested
                .api_url_override
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&api_url);
            let dialect = crate::ProviderProtocolDialect::detect_from_api_url(effective_api_url);
            let profile = match &requested.provider_profile_update {
                ProviderProfileUpdate::Unchanged => {
                    let profile = preserved.as_ref().ok_or_else(|| {
                        format!(
                            "模型 {} 尚无可保留的 Provider Profile，请显式选择通用兼容或已注册 Profile。",
                            requested.display_name
                        )
                    })?;
                    let preview = ModelConfigRecord {
                        id: config_id.clone(),
                        provider_model_id: requested.provider_model_id.clone(),
                        display_name: requested.display_name.clone(),
                        api_url_override: requested.api_url_override.clone(),
                        api_token_override: None,
                        supports_image: requested.supports_image,
                        context_window_tokens: requested.context_window_tokens,
                        provider_profile_config: profile.clone(),
                        input_price: requested.input_price.clone(),
                        cached_input_price: requested.cached_input_price.clone(),
                        output_price: requested.output_price.clone(),
                        enabled: requested.enabled,
                    };
                    validate_unchanged_profile(&preview, preserved.as_ref(), dialect)?;
                    profile.clone()
                }
                ProviderProfileUpdate::SelectGeneric => {
                    crate::ProviderProfileConfig::generic_for_dialect(dialect)
                }
                ProviderProfileUpdate::SelectRegisteredProfile {
                    profile_id,
                    settings: public_settings,
                } => {
                    let registration =
                        crate::resolve_ui_selectable_provider_registration(*profile_id, dialect)
                            .map_err(|error| {
                                format!(
                                    "模型 {} 的 Provider Profile 选择无效：{error}",
                                    requested.display_name
                                )
                            })?;
                    registration
                        .config_from_public_settings(*public_settings)
                        .map_err(|error| {
                            format!(
                                "模型 {} 的 Provider 设置无效：{error}",
                                requested.display_name
                            )
                        })?
                }
                ProviderProfileUpdate::SelectVendor {
                    vendor_id,
                    settings: public_settings,
                } => {
                    let registration = crate::resolve_provider_vendor_registration(
                        *vendor_id,
                        &requested.provider_model_id,
                        dialect,
                    )
                    .map_err(|error| {
                        format!(
                            "模型 {} 的 Provider 厂商选择无效：{error}",
                            requested.display_name
                        )
                    })?;
                    registration
                        .config_from_vendor_settings(*public_settings)
                        .map_err(|error| {
                            format!(
                                "模型 {} 的 Provider 设置无效：{error}",
                                requested.display_name
                            )
                        })?
                }
            };
            let override_mutation = std::mem::replace(
                &mut requested.api_token_override_mutation,
                CredentialMutation::Keep,
            );
            let planned_override = self.plan_credential_mutation(
                override_mutation,
                existing_model.and_then(|model| model.api_token_override_ref.as_deref()),
                &mut replacements,
            )?;
            let model = requested.into_record(
                config_id,
                profile,
                planned_override.validation_value.clone(),
            );
            stored_models.push(stored_model_from_resolved(
                &model,
                planned_override.credential_ref,
            ));
            models.push(model);
        }

        let settings = ModelSettingsRecord {
            api_url: api_url.trim().to_string(),
            api_token: planned_api_token.validation_value.unwrap_or_default(),
            search_mode,
            tavily_api_key: planned_tavily_api_key.validation_value.unwrap_or_default(),
            models,
        };
        validate_model_settings_fields(&settings)?;
        let stored = StoredModelSettingsRecord {
            api_url: settings.api_url,
            api_token_ref: planned_api_token.credential_ref,
            search_mode: settings.search_mode,
            tavily_api_key_ref: planned_tavily_api_key.credential_ref,
            models: stored_models,
        };
        let committed = self.commit_model_settings_plan(stored, replacements, existing)?;
        self.model_settings_editor_record(&committed)
            .map_err(Into::into)
    }

    fn resolve_model_settings(
        &self,
        stored: &StoredModelSettingsRecord,
    ) -> Result<ModelSettingsRecord, String> {
        Ok(ModelSettingsRecord {
            api_url: stored.api_url.clone(),
            api_token: self
                .resolve_credential(stored.api_token_ref.as_deref())?
                .unwrap_or_default(),
            search_mode: stored.search_mode.clone(),
            tavily_api_key: self
                .resolve_credential(stored.tavily_api_key_ref.as_deref())?
                .unwrap_or_default(),
            models: stored
                .models
                .iter()
                .map(|model| {
                    Ok(ModelConfigRecord {
                        id: model.id.clone(),
                        provider_model_id: model.provider_model_id.clone(),
                        display_name: model.display_name.clone(),
                        api_url_override: model.api_url_override.clone(),
                        api_token_override: self
                            .resolve_credential(model.api_token_override_ref.as_deref())?,
                        supports_image: model.supports_image,
                        context_window_tokens: model.context_window_tokens,
                        provider_profile_config: model.provider_profile_config.clone(),
                        input_price: model.input_price.clone(),
                        cached_input_price: model.cached_input_price.clone(),
                        output_price: model.output_price.clone(),
                        enabled: model.enabled,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        })
    }

    fn resolve_model_settings_snapshot(
        &self,
        stored: &StoredModelSettingsSnapshot,
    ) -> Result<ModelSettingsSnapshot, String> {
        Ok(ModelSettingsSnapshot {
            settings: self.resolve_model_settings(&stored.settings)?,
            configuration_revision: stored.configuration_revision.clone(),
            provider_connection_revisions: stored.provider_connection_revisions.clone(),
            provider_protocol_revisions: stored.provider_protocol_revisions.clone(),
            search_connection_revision: stored.search_connection_revision.clone(),
        })
    }

    fn resolve_model_settings_snapshot_for_model(
        &self,
        stored: &StoredModelSettingsSnapshot,
        model_config_id: &str,
        include_search: bool,
    ) -> Result<ModelSettingsSnapshot, String> {
        let stored_model = stored
            .settings
            .models
            .iter()
            .find(|model| model.id == model_config_id)
            .ok_or_else(|| "selected model configuration is unavailable".to_string())?;
        let uses_model_override = match (
            stored_model.api_url_override.as_deref(),
            stored_model.api_token_override_ref.as_deref(),
        ) {
            (Some(url), Some(_)) if !url.trim().is_empty() => true,
            (None, None) => false,
            _ => {
                return Err(
                    "selected model provider connection is incomplete or invalid".to_string(),
                )
            }
        };
        let global_api_token = if uses_model_override {
            String::new()
        } else {
            self.resolve_credential(stored.settings.api_token_ref.as_deref())?
                .unwrap_or_default()
        };
        let api_token_override = if uses_model_override {
            self.resolve_credential(stored_model.api_token_override_ref.as_deref())?
        } else {
            None
        };
        let tavily_api_key = if include_search && stored.settings.search_mode != "disabled" {
            self.resolve_credential(stored.settings.tavily_api_key_ref.as_deref())?
                .unwrap_or_default()
        } else {
            String::new()
        };
        let model = ModelConfigRecord {
            id: stored_model.id.clone(),
            provider_model_id: stored_model.provider_model_id.clone(),
            display_name: stored_model.display_name.clone(),
            api_url_override: stored_model.api_url_override.clone(),
            api_token_override,
            supports_image: stored_model.supports_image,
            context_window_tokens: stored_model.context_window_tokens,
            provider_profile_config: stored_model.provider_profile_config.clone(),
            input_price: stored_model.input_price.clone(),
            cached_input_price: stored_model.cached_input_price.clone(),
            output_price: stored_model.output_price.clone(),
            enabled: stored_model.enabled,
        };
        let provider_connection_revision = stored
            .provider_connection_revisions
            .get(model_config_id)
            .cloned()
            .ok_or_else(|| "selected model provider connection revision is missing".to_string())?;
        let provider_protocol_revision = stored
            .provider_protocol_revisions
            .get(model_config_id)
            .cloned()
            .ok_or_else(|| "selected model provider protocol revision is missing".to_string())?;
        Ok(ModelSettingsSnapshot {
            settings: ModelSettingsRecord {
                api_url: stored.settings.api_url.clone(),
                api_token: global_api_token,
                search_mode: stored.settings.search_mode.clone(),
                tavily_api_key,
                models: vec![model],
            },
            configuration_revision: stored.configuration_revision.clone(),
            provider_connection_revisions: BTreeMap::from([(
                model_config_id.to_string(),
                provider_connection_revision,
            )]),
            provider_protocol_revisions: BTreeMap::from([(
                model_config_id.to_string(),
                provider_protocol_revision,
            )]),
            search_connection_revision: stored.search_connection_revision.clone(),
        })
    }

    /// Builds a credential-free catalog snapshot while an existing SQLite transaction is held.
    /// Opaque references are reduced to a presence marker; native credential I/O is deliberately
    /// deferred until execution begins outside the database critical section.
    pub(crate) fn model_settings_catalog_snapshot_in_connection(
        &self,
        connection: &rusqlite::Connection,
    ) -> Result<Option<ModelSettingsSnapshot>, String> {
        let stored = config_repository::load_model_settings_snapshot_in_connection(connection)
            .map_err(storage_error)?;
        Ok(stored.as_ref().map(model_settings_catalog_snapshot))
    }

    fn resolve_credential(&self, credential_ref: Option<&str>) -> Result<Option<String>, String> {
        let Some(credential_ref) = credential_ref else {
            return Ok(None);
        };
        let reference = CredentialReference::parse(credential_ref)
            .map_err(|_| "provider credential reference is invalid".to_string())?;
        if !self.model_credentials.supports_reference(&reference) {
            return Err("provider credential belongs to an unavailable backend".to_string());
        }
        let secret = self
            .model_credentials
            .get(&reference)
            .map_err(|_| "provider credential store is unavailable".to_string())?
            .ok_or_else(|| "provider credential is unavailable".to_string())?;
        secret
            .with_secret_bytes(|bytes| {
                std::str::from_utf8(bytes)
                    .map(str::to_owned)
                    .map_err(|_| "provider credential is malformed".to_string())
            })
            .map(Some)
    }

    fn credential_status(&self, credential_ref: Option<&str>) -> CredentialStatus {
        let Some(credential_ref) = credential_ref else {
            return CredentialStatus::Missing;
        };
        let Ok(reference) = CredentialReference::parse(credential_ref) else {
            return CredentialStatus::Unavailable;
        };
        if !self.model_credentials.supports_reference(&reference) {
            return CredentialStatus::Unavailable;
        }
        match self.model_credentials.get(&reference) {
            Ok(Some(_)) => CredentialStatus::Configured,
            Ok(None) | Err(_) => CredentialStatus::Unavailable,
        }
    }

    fn model_settings_editor_record(
        &self,
        stored: &StoredModelSettingsSnapshot,
    ) -> Result<ModelSettingsEditorRecord, String> {
        Ok(ModelSettingsEditorRecord {
            configuration_revision: stored.configuration_revision.clone(),
            api_url: stored.settings.api_url.clone(),
            api_token_status: self.credential_status(stored.settings.api_token_ref.as_deref()),
            search_mode: stored.settings.search_mode.clone(),
            tavily_api_key_status: self
                .credential_status(stored.settings.tavily_api_key_ref.as_deref()),
            models: stored
                .settings
                .models
                .iter()
                .map(|model| ModelConfigEditorRecord {
                    id: model.id.clone(),
                    provider_model_id: model.provider_model_id.clone(),
                    display_name: model.display_name.clone(),
                    api_url_override: model.api_url_override.clone(),
                    api_token_override_status: self
                        .credential_status(model.api_token_override_ref.as_deref()),
                    supports_image: model.supports_image,
                    context_window_tokens: model.context_window_tokens,
                    provider_profile_config: model.provider_profile_config.clone(),
                    input_price: model.input_price.clone(),
                    cached_input_price: model.cached_input_price.clone(),
                    output_price: model.output_price.clone(),
                    enabled: model.enabled,
                })
                .collect(),
        })
    }

    fn plan_credential_mutation(
        &self,
        mutation: CredentialMutation,
        existing_ref: Option<&str>,
        replacements: &mut Vec<PlannedCredentialReplacement>,
    ) -> Result<PlannedCredentialField, String> {
        match mutation {
            CredentialMutation::Keep => Ok(PlannedCredentialField {
                credential_ref: existing_ref.map(str::to_owned),
                validation_value: existing_ref.map(|_| "configured-credential".to_string()),
            }),
            CredentialMutation::Clear => Ok(PlannedCredentialField {
                credential_ref: None,
                validation_value: None,
            }),
            CredentialMutation::Replace { value } => {
                validate_provider_credential(&value)?;
                let reference = self.model_credentials.new_reference();
                let credential_ref = reference.as_str().to_string();
                let secret = CredentialSecret::new(value)
                    .map_err(|_| "provider credential is invalid".to_string())?;
                replacements.push(PlannedCredentialReplacement { reference, secret });
                Ok(PlannedCredentialField {
                    credential_ref: Some(credential_ref),
                    validation_value: Some("configured-credential".to_string()),
                })
            }
        }
    }

    fn plan_resolved_model_settings(
        &self,
        settings: &ModelSettingsRecord,
        existing: Option<&StoredModelSettingsRecord>,
    ) -> Result<(StoredModelSettingsRecord, Vec<PlannedCredentialReplacement>), String> {
        let mut replacements = Vec::new();
        let api_token = self.plan_direct_credential(
            &settings.api_token,
            existing.and_then(|record| record.api_token_ref.as_deref()),
            &mut replacements,
        )?;
        let tavily_api_key = self.plan_direct_credential(
            &settings.tavily_api_key,
            existing.and_then(|record| record.tavily_api_key_ref.as_deref()),
            &mut replacements,
        )?;
        let models = settings
            .models
            .iter()
            .map(|model| {
                let previous_ref = existing
                    .and_then(|record| record.models.iter().find(|item| item.id == model.id))
                    .and_then(|item| item.api_token_override_ref.as_deref());
                let planned = self.plan_direct_optional_credential(
                    model.api_token_override.as_deref(),
                    previous_ref,
                    &mut replacements,
                )?;
                Ok(stored_model_from_resolved(model, planned.credential_ref))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((
            StoredModelSettingsRecord {
                api_url: settings.api_url.trim().to_string(),
                api_token_ref: api_token.credential_ref,
                search_mode: settings.search_mode.clone(),
                tavily_api_key_ref: tavily_api_key.credential_ref,
                models,
            },
            replacements,
        ))
    }

    fn plan_direct_optional_credential(
        &self,
        value: Option<&str>,
        existing_ref: Option<&str>,
        replacements: &mut Vec<PlannedCredentialReplacement>,
    ) -> Result<PlannedCredentialField, String> {
        match value.filter(|value| !value.is_empty()) {
            Some(value) => self.plan_direct_credential(value, existing_ref, replacements),
            None => {
                self.plan_credential_mutation(CredentialMutation::Clear, existing_ref, replacements)
            }
        }
    }

    fn plan_direct_credential(
        &self,
        value: &str,
        existing_ref: Option<&str>,
        replacements: &mut Vec<PlannedCredentialReplacement>,
    ) -> Result<PlannedCredentialField, String> {
        // This compatibility-only, Host-internal path historically normalized surrounding
        // whitespace. Renderer mutations remain strict and never trim credential material.
        let value = value.trim();
        if value.is_empty() {
            return self.plan_credential_mutation(
                CredentialMutation::Clear,
                existing_ref,
                replacements,
            );
        }
        if let Some(existing_ref) = existing_ref {
            let reference = CredentialReference::parse(existing_ref)
                .map_err(|_| "provider credential reference is invalid".to_string())?;
            if self.model_credentials.supports_reference(&reference) {
                if let Ok(Some(secret)) = self.model_credentials.get(&reference) {
                    if secret.with_secret_bytes(|bytes| bytes == value.as_bytes()) {
                        return self.plan_credential_mutation(
                            CredentialMutation::Keep,
                            Some(existing_ref),
                            replacements,
                        );
                    }
                }
            }
        }
        self.plan_credential_mutation(
            CredentialMutation::Replace {
                value: value.to_string(),
            },
            existing_ref,
            replacements,
        )
    }

    fn commit_model_settings_plan(
        &self,
        stored: StoredModelSettingsRecord,
        replacements: Vec<PlannedCredentialReplacement>,
        existing: Option<&StoredModelSettingsRecord>,
    ) -> Result<StoredModelSettingsSnapshot, String> {
        let staged_refs = replacements
            .iter()
            .map(|replacement| replacement.reference.as_str().to_string())
            .collect::<Vec<_>>();
        {
            let mut connection = self.state.connection()?;
            config_repository::stage_model_provider_credentials(&mut connection, &staged_refs)
                .map_err(storage_error)?;
        }

        for replacement in replacements {
            let reference = replacement.reference;
            if let Err(_error) = self
                .model_credentials
                .replace(&reference, replacement.secret)
            {
                for staged in &staged_refs {
                    let Ok(staged_reference) = CredentialReference::parse(staged) else {
                        continue;
                    };
                    if !self.model_credentials.supports_reference(&staged_reference) {
                        continue;
                    }
                    if matches!(
                        self.model_credentials.delete(&staged_reference),
                        Ok(CredentialDeleteOutcome::Deleted | CredentialDeleteOutcome::NotFound)
                    ) {
                        if let Ok(connection) = self.state.connection() {
                            let _ = config_repository::remove_model_provider_credential_staging(
                                &connection,
                                staged,
                            );
                        }
                    }
                }
                return Err("provider credential store is unavailable".to_string());
            }
        }

        let active_refs = model_credential_refs(&stored);
        let retired_refs = existing
            .map(model_credential_refs)
            .unwrap_or_default()
            .difference(&active_refs)
            .cloned()
            .collect::<Vec<_>>();
        let committed = {
            let mut connection = self.state.connection()?;
            config_repository::save_model_settings_with_credential_journal(
                &mut connection,
                stored,
                &staged_refs,
                &retired_refs,
            )
            .map_err(storage_error)?
        };

        for retired in retired_refs {
            let Ok(reference) = CredentialReference::parse(&retired) else {
                continue;
            };
            if self.model_credentials.supports_reference(&reference)
                && matches!(
                    self.model_credentials.delete(&reference),
                    Ok(CredentialDeleteOutcome::Deleted | CredentialDeleteOutcome::NotFound)
                )
            {
                if let Ok(connection) = self.state.connection() {
                    let _ = config_repository::remove_model_provider_credential_cleanup(
                        &connection,
                        &retired,
                    );
                }
            }
        }
        Ok(committed)
    }

    /// Completes or rolls back credential operations interrupted around the SQLite commit.
    pub fn reconcile_model_provider_credentials(
        &self,
    ) -> Result<ModelProviderCredentialReconciliationReport, String> {
        let _guard = self
            .model_credential_lock
            .lock()
            .map_err(|_| "model credential coordinator is unavailable".to_string())?;
        let mut report = ModelProviderCredentialReconciliationReport::default();
        let staged_records = {
            let connection = self.state.connection()?;
            config_repository::list_model_provider_credential_staging(&connection)
                .map_err(storage_error)?
        };
        for staged in staged_records {
            if staged.is_active {
                let connection = self.state.connection()?;
                config_repository::remove_model_provider_credential_staging(
                    &connection,
                    &staged.credential_ref,
                )
                .map_err(storage_error)?;
                report.completed_staging += 1;
                continue;
            }
            let reference = CredentialReference::parse(&staged.credential_ref)
                .map_err(|_| "provider credential staging reference is invalid".to_string())?;
            if !self.model_credentials.supports_reference(&reference) {
                return Err(
                    "provider credential staging belongs to an unavailable backend".to_string(),
                );
            }
            self.model_credentials
                .delete(&reference)
                .map_err(|_| "provider credential store is unavailable".to_string())?;
            let connection = self.state.connection()?;
            config_repository::remove_model_provider_credential_staging(
                &connection,
                &staged.credential_ref,
            )
            .map_err(storage_error)?;
            report.removed_orphaned_credentials += 1;
        }
        let cleanup_records = {
            let connection = self.state.connection()?;
            config_repository::list_model_provider_credential_cleanup(&connection)
                .map_err(storage_error)?
        };
        for retired in cleanup_records {
            if retired.is_active {
                let connection = self.state.connection()?;
                config_repository::remove_model_provider_credential_cleanup(
                    &connection,
                    &retired.credential_ref,
                )
                .map_err(storage_error)?;
                continue;
            }
            let reference = CredentialReference::parse(&retired.credential_ref)
                .map_err(|_| "provider credential cleanup reference is invalid".to_string())?;
            if !self.model_credentials.supports_reference(&reference) {
                return Err(
                    "provider credential cleanup belongs to an unavailable backend".to_string(),
                );
            }
            self.model_credentials
                .delete(&reference)
                .map_err(|_| "provider credential store is unavailable".to_string())?;
            let connection = self.state.connection()?;
            config_repository::remove_model_provider_credential_cleanup(
                &connection,
                &retired.credential_ref,
            )
            .map_err(storage_error)?;
            report.removed_retired_credentials += 1;
        }
        Ok(report)
    }

    pub fn load_agent_prompt_preferences(&self) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::load_agent_prompt_preferences(&connection)
            .map_err(storage_error)
    }

    pub fn save_agent_prompt_preferences(
        &self,
        preferences: AgentPromptPreferencesRecord,
    ) -> Result<AgentPromptPreferencesRecord, String> {
        let connection = self.state.connection()?;
        agent_prompt_preferences_repository::save_agent_prompt_preferences(&connection, preferences)
            .map_err(storage_error)
    }

    pub fn load_composer_drafts(&self) -> Result<Vec<ComposerDraftRecord>, String> {
        let connection = self.state.connection()?;
        composer_draft_repository::list_composer_drafts(&connection)
            .map_err(storage_error)
            .and_then(|drafts| {
                drafts
                    .into_iter()
                    .map(|draft| {
                        draft.validate_current_payloads()?;
                        Ok(draft.normalize_permission_mode())
                    })
                    .collect()
            })
    }

    pub fn save_composer_draft(
        &self,
        draft: ComposerDraftRecord,
    ) -> Result<ComposerDraftRecord, String> {
        draft.validate_current_payloads()?;
        let draft = draft.normalize_permission_mode();
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, draft.project_id.as_deref())?;
        composer_draft_repository::save_composer_draft(&connection, draft.clone())
            .map_err(storage_error)?;
        Ok(draft)
    }

    pub fn save_composer_draft_message(
        &self,
        scope_id: &str,
        message: &str,
        updated_at: i64,
    ) -> Result<bool, String> {
        if scope_id.trim().is_empty() || updated_at < 0 {
            return Err("stored_composer_draft_malformed".to_string());
        }
        let connection = self.state.connection()?;
        composer_draft_repository::save_composer_draft_message(
            &connection,
            scope_id,
            message,
            updated_at,
        )
        .map_err(storage_error)
    }

    /// Loads effective enablement for a batch of complete, opaque Skill ids.
    ///
    /// An absent override is intentionally enabled by default. The returned
    /// map contains one entry for every distinct requested id.
    pub fn load_skill_enablement(
        &self,
        skill_ids: &[String],
    ) -> Result<BTreeMap<String, bool>, String> {
        for skill_id in skill_ids {
            validate_skill_enablement_id(skill_id)?;
        }
        let mut enablement = skill_ids
            .iter()
            .cloned()
            .map(|skill_id| (skill_id, true))
            .collect::<BTreeMap<_, _>>();
        if enablement.is_empty() {
            return Ok(enablement);
        }

        let mut connection = self.state.connection()?;
        let overrides = skill_enablement_repository::load_skill_enablement_overrides(
            &mut connection,
            skill_ids,
        )
        .map_err(storage_error)?;
        enablement.extend(overrides);
        Ok(enablement)
    }

    /// Loads effective enablement together with its monotonic mutation
    /// generation for compare-and-swap state tokens.
    pub fn load_skill_enablement_states(
        &self,
        skill_ids: &[String],
    ) -> Result<BTreeMap<String, skill_enablement_repository::SkillEnablementState>, String> {
        for skill_id in skill_ids {
            validate_skill_enablement_id(skill_id)?;
        }
        let mut connection = self.state.connection()?;
        skill_enablement_repository::load_skill_enablement_states(&mut connection, skill_ids)
            .map_err(storage_error)
    }

    /// Stores an explicit enablement override and reports whether state changed.
    pub fn set_skill_enablement_override(
        &self,
        skill_id: &str,
        enabled: bool,
    ) -> Result<bool, String> {
        validate_skill_enablement_id(skill_id)?;
        let connection = self.state.connection()?;
        skill_enablement_repository::set_skill_enablement_override(&connection, skill_id, enabled)
            .map_err(storage_error)
    }

    /// Atomically mutates effective enablement if it still matches the state
    /// observed by the caller. An absent row participates as the product
    /// default (`enabled = true`).
    pub fn compare_and_set_skill_enablement(
        &self,
        skill_id: &str,
        expected_enabled: bool,
        expected_generation: u64,
        target: bool,
    ) -> Result<skill_enablement_repository::SkillEnablementCompareAndSetOutcome, String> {
        validate_skill_enablement_id(skill_id)?;
        let mut connection = self.state.connection()?;
        skill_enablement_repository::compare_and_set_skill_enablement(
            &mut connection,
            skill_id,
            expected_enabled,
            expected_generation,
            target,
        )
        .map_err(storage_error)
    }

    /// Removes an explicit override, restoring the default enabled state.
    pub fn delete_skill_enablement_override(&self, skill_id: &str) -> Result<bool, String> {
        validate_skill_enablement_id(skill_id)?;
        let connection = self.state.connection()?;
        skill_enablement_repository::delete_skill_enablement_override(&connection, skill_id)
            .map_err(storage_error)
    }

    pub fn load_ui_preferences(&self) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::load_ui_preferences(&connection).map_err(storage_error)
    }

    pub fn save_ui_preferences(
        &self,
        preferences: UiPreferencesRecord,
    ) -> Result<UiPreferencesRecord, String> {
        let connection = self.state.connection()?;
        preferences_repository::save_ui_preferences(&connection, preferences).map_err(storage_error)
    }

    pub fn upsert_agent_usage(&self, record: AgentUsageRecordInsert) -> Result<(), String> {
        let connection = self.state.connection()?;
        usage_repository::upsert_usage_record(&connection, &record).map_err(storage_error)
    }

    pub fn load_agent_usage_for_owner(
        &self,
        run_id: &str,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<Option<AgentUsageRecordInsert>, String> {
        let connection = self.state.connection()?;
        usage_repository::load_usage_record_for_owner(
            &connection,
            run_id,
            conversation_id,
            message_id,
        )
        .map_err(storage_error)
    }

    pub fn get_usage_summary(
        &self,
        input: &AgentUsageSummaryInput,
        now_ms: i64,
    ) -> Result<AgentUsageSummaryOutput, String> {
        let connection = self.state.connection()?;
        usage_repository::usage_summary(&connection, input, now_ms).map_err(storage_error)
    }

    pub fn clear_usage_records(
        &self,
        input: &AgentUsageClearInput,
    ) -> Result<AgentUsageClearOutput, String> {
        let connection = self.state.connection()?;
        usage_repository::clear_usage_records(&connection, input).map_err(storage_error)
    }

    pub fn estimate_usage_cost(
        &self,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        input_price: &str,
        cached_input_price: &str,
        output_price: &str,
    ) -> Option<f64> {
        usage_repository::estimate_usage_cost(
            input_tokens,
            cached_input_tokens,
            output_tokens,
            input_price,
            cached_input_price,
            output_price,
        )
    }
}

fn validate_unchanged_profile(
    model: &ModelConfigRecord,
    existing_config: Option<&crate::ProviderProfileConfig>,
    dialect: crate::ProviderProtocolDialect,
) -> Result<(), String> {
    let config = &model.provider_profile_config;
    let exactly_preserved = existing_config == Some(config);

    match config.validate() {
        Ok(()) => config
            .validate_for_model(&model.provider_model_id, dialect)
            .map_err(|error| {
                format!(
                    "模型 {} 的 Provider Profile 与当前接口协议不兼容：{error}",
                    model.display_name
                )
            }),
        Err(_) if exactly_preserved => Ok(()),
        Err(error) => Err(format!(
            "模型 {} 的 Provider Profile 无效：{error}",
            model.display_name
        )),
    }
}
