use super::*;

pub(super) const MAX_SKILL_ENABLEMENT_ID_BYTES: usize = 16 * 1024;

pub(super) fn validate_model_settings(settings: &ModelSettingsRecord) -> Result<(), String> {
    let mut model_ids = HashSet::new();
    for model in &settings.models {
        let model_id = model.id.trim();
        if model_id.is_empty() {
            return Err("模型 ID 不能为空。".to_string());
        }
        if !model_ids.insert(model_id) {
            return Err(format!("模型 ID 重复：{model_id}"));
        }
        if model.context_window_tokens == Some(0) {
            return Err(format!("模型 {model_id} 的上下文窗口必须大于 0。"));
        }
        model.connection_override()?;
        if !usage_repository::is_valid_price_per_1k(&model.input_price) {
            return Err(format!(
                "模型 {model_id} 的输入价格必须是大于或等于 0 的有效数字。"
            ));
        }
        if !usage_repository::is_valid_price_per_1k(&model.output_price) {
            return Err(format!(
                "模型 {model_id} 的输出价格必须是大于或等于 0 的有效数字。"
            ));
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

    pub fn list_image_generation_credential_cleanup(&self) -> Result<Vec<String>, String> {
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
        let mut connection = self.state.connection()?;
        config_repository::load_model_settings(&mut connection).map_err(storage_error)
    }

    pub fn load_model_settings_snapshot(&self) -> Result<Option<ModelSettingsSnapshot>, String> {
        let mut connection = self.state.connection()?;
        config_repository::load_model_settings_snapshot(&mut connection).map_err(storage_error)
    }

    pub fn save_model_settings(&self, settings: ModelSettingsRecord) -> Result<(), String> {
        validate_model_settings(&settings)?;
        let mut connection = self.state.connection()?;
        config_repository::save_model_settings(&mut connection, settings).map_err(storage_error)
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
            .map(|drafts| {
                drafts
                    .into_iter()
                    .map(ComposerDraftRecord::normalize_permission_mode)
                    .collect()
            })
            .map_err(storage_error)
    }

    pub fn save_composer_draft(
        &self,
        draft: ComposerDraftRecord,
    ) -> Result<ComposerDraftRecord, String> {
        let draft = draft.normalize_permission_mode();
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, draft.project_id.as_deref())?;
        composer_draft_repository::save_composer_draft(&connection, draft.clone())
            .map_err(storage_error)?;
        Ok(draft)
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
        output_tokens: Option<u64>,
        input_price: &str,
        output_price: &str,
    ) -> Option<f64> {
        usage_repository::estimate_usage_cost(
            input_tokens,
            output_tokens,
            input_price,
            output_price,
        )
    }
}
