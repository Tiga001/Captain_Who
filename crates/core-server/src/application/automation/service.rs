use super::permissions::{
    ensure_automation_permission_mode_enabled, reasoning_projection, resolve_automation_permissions,
};
use super::schedule::{normalize_schedule, AutomationScheduleError};
use super::scheduler::AutomationSchedulerWake;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use mycopilot_core::storage::automation_repository::{
    AutomationAttentionListCursor, AutomationAttentionRecord, AutomationCompareAndSetOutcome,
    AutomationConfigRecord, AutomationCreateOutcome, AutomationEventRecord, AutomationListCursor,
    AutomationListInput, AutomationRecord, AutomationRunEnqueueOutcome, AutomationRunListCursor,
    AutomationRunRecord, NewAutomationRecord, NewManualAutomationRunRecord,
    StoredAutomationRunStatus, StoredAutomationStatus,
};
use mycopilot_core::storage::models::{ModelConfigRecord, ProjectRecord};
use mycopilot_core::storage::now_ms;
use mycopilot_core::storage::service::StorageService;
use mycopilot_protocol_rs::{
    AutomationAttentionAcknowledgeInputDto, AutomationAttentionAcknowledgeOutputDto,
    AutomationAttentionDto, AutomationAttentionKindDto, AutomationAttentionSummaryInputDto,
    AutomationAttentionSummaryOutputDto, AutomationBlockedCodeDto, AutomationCreateInputDto,
    AutomationDeleteInputDto, AutomationDeleteOutputDto, AutomationDestinationDto,
    AutomationDestinationInputDto, AutomationErrorDataDto, AutomationErrorKindDto,
    AutomationErrorTypeDto, AutomationEventDto, AutomationEventKindDto, AutomationGetInputDto,
    AutomationHealthDto, AutomationListCountsDto, AutomationListInputDto, AutomationListOutputDto,
    AutomationNotificationPolicyDto, AutomationPermissionModeDto, AutomationProjectBindingDto,
    AutomationReportKindDto, AutomationRunDto, AutomationRunNowInputDto, AutomationRunStatusDto,
    AutomationRunTriggerKindDto, AutomationRunsListInputDto, AutomationRunsListOutputDto,
    AutomationSetEnabledInputDto, AutomationStatusDto, AutomationTargetSnapshotDto,
    AutomationTaskDto, AutomationUpdateInputDto, AUTOMATION_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAX_ID_BYTES: usize = 256;
const MAX_TITLE_BYTES: usize = 512;
const MAX_PROMPT_BYTES: usize = 65_536;
const MAX_QUERY_BYTES: usize = 512;
const MAX_PAGE_SIZE: u32 = 100;

/// Immutable execution authority captured when a run is enqueued. This is intentionally distinct
/// from the Renderer task DTO: workers never reconstruct authority from a later task revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AutomationConfigSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) automation_id: String,
    pub(crate) config_revision: i64,
    pub(crate) title: String,
    pub(crate) prompt: String,
    pub(crate) destination_kind: String,
    pub(crate) target_conversation_id: Option<String>,
    pub(crate) project_binding_kind: String,
    pub(crate) project_id: Option<String>,
    pub(crate) model_id: Option<String>,
    pub(crate) permission_mode: String,
    pub(crate) permission_mode_version: i64,
    pub(crate) permissions: mycopilot_protocol_rs::AutomationResolvedPermissionsDto,
    pub(crate) reasoning: Option<mycopilot_protocol_rs::AutomationReasoningProjectionDto>,
    pub(crate) schedule: mycopilot_protocol_rs::AutomationScheduleDto,
    pub(crate) rrule: String,
    pub(crate) timezone: String,
    pub(crate) notification_policy: String,
}

impl AutomationConfigSnapshot {
    pub(crate) fn parse(value: &str) -> Result<Self, AutomationServiceError> {
        let snapshot: Self = serde_json::from_str(value)
            .map_err(|_| AutomationServiceError::internal("Stored run snapshot is invalid"))?;
        validate_schema(snapshot.schema_version)?;
        validate_id(&snapshot.automation_id, "automationId")?;
        if snapshot.config_revision <= 0 {
            return Err(AutomationServiceError::internal(
                "Stored run snapshot revision is invalid",
            ));
        }
        validate_title_prompt(&snapshot.title, &snapshot.prompt)?;
        Ok(snapshot)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AutomationService<'a> {
    storage: &'a StorageService,
    scheduler_wake: Option<&'a AutomationSchedulerWake>,
}

impl<'a> AutomationService<'a> {
    pub(crate) fn new(storage: &'a StorageService) -> Self {
        Self {
            storage,
            scheduler_wake: None,
        }
    }

    pub(crate) fn with_scheduler_wake(
        storage: &'a StorageService,
        scheduler_wake: &'a AutomationSchedulerWake,
    ) -> Self {
        Self {
            storage,
            scheduler_wake: Some(scheduler_wake),
        }
    }

    fn wake_scheduler(&self) {
        if let Some(wake) = self.scheduler_wake {
            wake.wake();
        }
    }

    pub(crate) fn list(
        &self,
        input: AutomationListInputDto,
    ) -> Result<AutomationListOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_limit(input.limit)?;
        if let Some(query) = &input.query {
            validate_bounded(query, MAX_QUERY_BYTES, true, "query")?;
        }
        let cursor = input
            .cursor
            .as_deref()
            .map(decode_task_cursor)
            .transpose()?;
        let page = self
            .storage
            .list_automations(&AutomationListInput {
                status: input.status.map(stored_status),
                query: input.query,
                cursor,
                limit: input.limit as usize,
            })
            .map_err(AutomationServiceError::internal)?;
        let tasks = page
            .items
            .iter()
            .map(|record| self.task_dto(record))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AutomationListOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            tasks,
            next_cursor: page
                .next_cursor
                .as_ref()
                .map(encode_task_cursor)
                .transpose()?,
            counts: AutomationListCountsDto {
                all: page.total_count,
                active: page.active_count,
                paused: page.paused_count,
            },
            attention_count: page.attention_count,
            last_sequence: nonnegative_u64(page.last_event_sequence)?,
        })
    }

    pub(crate) fn get(
        &self,
        input: AutomationGetInputDto,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        let record = self
            .storage
            .get_automation(&input.automation_id)
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| AutomationServiceError::not_found(Some(input.automation_id)))?;
        self.task_dto(&record)
    }

    pub(crate) fn create(
        &self,
        input: AutomationCreateInputDto,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.request_id, "requestId")?;
        if let Some(existing) = self
            .storage
            .get_automation_by_create_request_id(&input.request_id)
            .map_err(AutomationServiceError::internal)?
        {
            return self.task_dto(&existing);
        }
        validate_title_prompt(&input.title, &input.prompt)?;
        validate_notification_policy(&input.destination, input.notification_policy)?;
        let status = stored_status(input.status);
        let config = self.build_config(
            input.title,
            input.prompt,
            input.destination,
            input.permission_mode,
            input.permission_mode_version,
            input.schedule,
            input.notification_policy,
            status,
        )?;
        let outcome = self
            .storage
            .create_automation(&NewAutomationRecord {
                id: crate::application::agent_support::create_id("automation"),
                create_request_id: input.request_id,
                status,
                config,
            })
            .map_err(AutomationServiceError::internal)?;
        let record = match outcome {
            AutomationCreateOutcome::Created(record)
            | AutomationCreateOutcome::Existing(record) => record,
        };
        self.wake_scheduler();
        self.task_dto(&record)
    }

    pub(crate) fn update(
        &self,
        input: AutomationUpdateInputDto,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        validate_title_prompt(&input.title, &input.prompt)?;
        validate_notification_policy(&input.destination, input.notification_policy)?;
        let expected_revision = revision_i64(input.expected_revision)?;
        let existing = self
            .storage
            .get_automation(&input.automation_id)
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| AutomationServiceError::not_found(Some(input.automation_id.clone())))?;
        if existing.revision != expected_revision {
            return Err(AutomationServiceError::revision_conflict(&existing));
        }
        let config = self.build_config(
            input.title,
            input.prompt,
            input.destination,
            input.permission_mode,
            input.permission_mode_version,
            input.schedule,
            input.notification_policy,
            existing.status,
        )?;
        let outcome = self
            .storage
            .replace_automation_config(&input.automation_id, expected_revision, &config)
            .map_err(AutomationServiceError::internal)?;
        let task = self.cas_task(outcome, &input.automation_id)?;
        self.wake_scheduler();
        Ok(task)
    }

    pub(crate) fn set_enabled(
        &self,
        input: AutomationSetEnabledInputDto,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        let expected_revision = revision_i64(input.expected_revision)?;
        let existing = self
            .storage
            .get_automation(&input.automation_id)
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| AutomationServiceError::not_found(Some(input.automation_id.clone())))?;
        if existing.revision != expected_revision {
            return Err(AutomationServiceError::revision_conflict(&existing));
        }
        let status = if input.enabled {
            StoredAutomationStatus::Active
        } else {
            StoredAutomationStatus::Paused
        };
        let next_run_at = if input.enabled && existing.config.health_state == "ok" {
            let schedule = parse_schedule(&existing.config.schedule_json)?;
            Some(
                normalize_schedule(&schedule)
                    .map_err(AutomationServiceError::schedule)?
                    .next_run_at_ms(now_ms())
                    .map_err(AutomationServiceError::schedule)?,
            )
        } else {
            None
        };
        let outcome = self
            .storage
            .set_automation_status(&input.automation_id, expected_revision, status, next_run_at)
            .map_err(AutomationServiceError::internal)?;
        let task = self.cas_task(outcome, &input.automation_id)?;
        self.wake_scheduler();
        Ok(task)
    }

    pub(crate) fn delete(
        &self,
        input: AutomationDeleteInputDto,
    ) -> Result<AutomationDeleteOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        let expected_revision = revision_i64(input.expected_revision)?;
        let outcome = self
            .storage
            .tombstone_automation(&input.automation_id, expected_revision)
            .map_err(AutomationServiceError::internal)?;
        match outcome {
            AutomationCompareAndSetOutcome::Updated(record) => {
                self.wake_scheduler();
                Ok(AutomationDeleteOutputDto {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    automation_id: record.id,
                    deleted_at: record.deleted_at.unwrap_or(record.updated_at),
                })
            }
            AutomationCompareAndSetOutcome::RevisionConflict(record) => {
                Err(AutomationServiceError::revision_conflict(&record))
            }
            AutomationCompareAndSetOutcome::NotFound => {
                Err(AutomationServiceError::not_found(Some(input.automation_id)))
            }
        }
    }

    pub(crate) fn run_now(
        &self,
        input: AutomationRunNowInputDto,
    ) -> Result<AutomationRunDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        validate_id(&input.request_id, "requestId")?;
        if let Some(existing) = self
            .storage
            .get_automation_run_by_manual_request_id(&input.request_id)
            .map_err(AutomationServiceError::internal)?
        {
            if existing.automation_id != input.automation_id {
                return Err(AutomationServiceError::validation(
                    "requestId is already associated with another automation.",
                    Some("requestId"),
                ));
            }
            return run_dto(&existing);
        }
        let task = self
            .storage
            .get_automation(&input.automation_id)
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| AutomationServiceError::not_found(Some(input.automation_id.clone())))?;
        let expected_revision = task.revision;
        if task.config.health_state != "ok" {
            let message = task
                .config
                .blocked_message
                .unwrap_or_else(|| "The automation configuration is blocked.".to_string());
            return Err(
                if task.config.blocked_code.as_deref() == Some("permission_disabled") {
                    AutomationServiceError::permission_disabled(message)
                } else {
                    AutomationServiceError::target_invalid(Some(task.id), message)
                },
            );
        }
        if let Some(block) = self.current_target_block(&task)? {
            let outcome = self
                .storage
                .block_automation(&task.id, task.revision, block.code, block.message)
                .map_err(AutomationServiceError::internal)?;
            return Err(match outcome {
                AutomationCompareAndSetOutcome::Updated(record) => {
                    AutomationServiceError::target_invalid(Some(record.id), block.message)
                }
                AutomationCompareAndSetOutcome::RevisionConflict(record) => {
                    AutomationServiceError::revision_conflict(&record)
                }
                AutomationCompareAndSetOutcome::NotFound => {
                    AutomationServiceError::not_found(Some(task.id))
                }
            });
        }
        let preferences = self
            .storage
            .load_ui_preferences()
            .map_err(AutomationServiceError::internal)?;
        if let Err(error) = ensure_automation_permission_mode_enabled(
            parse_permission_mode(&task.config.permission_mode)?,
            &preferences,
        ) {
            let message = error.to_string();
            let outcome = self
                .storage
                .block_automation(&task.id, task.revision, "permission_disabled", &message)
                .map_err(AutomationServiceError::internal)?;
            self.wake_scheduler();
            return Err(match outcome {
                AutomationCompareAndSetOutcome::Updated(_) => {
                    AutomationServiceError::permission_disabled(message)
                }
                AutomationCompareAndSetOutcome::RevisionConflict(record) => {
                    AutomationServiceError::revision_conflict(&record)
                }
                AutomationCompareAndSetOutcome::NotFound => {
                    AutomationServiceError::not_found(Some(task.id))
                }
            });
        }
        let snapshot = config_snapshot(&task)?;
        let outcome = self
            .storage
            .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
                id: crate::application::agent_support::create_id("automation-run"),
                automation_id: task.id.clone(),
                manual_request_id: input.request_id,
                scheduled_for: now_ms(),
                config_revision: task.revision,
                config_snapshot_json: snapshot,
                expected_revision,
            })
            .map_err(AutomationServiceError::internal)?;
        match outcome {
            AutomationRunEnqueueOutcome::Enqueued(run)
            | AutomationRunEnqueueOutcome::Existing(run) => {
                self.wake_scheduler();
                run_dto(&run)
            }
            AutomationRunEnqueueOutcome::ActiveConflict(run) => {
                Err(AutomationServiceError::active_run(&task.id, &run.id))
            }
            AutomationRunEnqueueOutcome::RevisionConflict(record) => {
                Err(AutomationServiceError::revision_conflict(&record))
            }
            AutomationRunEnqueueOutcome::AutomationNotFound => {
                Err(AutomationServiceError::not_found(Some(task.id)))
            }
        }
    }

    pub(crate) fn list_runs(
        &self,
        input: AutomationRunsListInputDto,
    ) -> Result<AutomationRunsListOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.automation_id, "automationId")?;
        validate_limit(input.limit)?;
        if self
            .storage
            .get_automation(&input.automation_id)
            .map_err(AutomationServiceError::internal)?
            .is_none()
        {
            return Err(AutomationServiceError::not_found(Some(input.automation_id)));
        }
        let cursor = input.cursor.as_deref().map(decode_run_cursor).transpose()?;
        let page = self
            .storage
            .list_automation_runs(&input.automation_id, cursor.as_ref(), input.limit as usize)
            .map_err(AutomationServiceError::internal)?;
        Ok(AutomationRunsListOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            automation_id: input.automation_id,
            runs: page
                .items
                .iter()
                .map(run_dto)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor: page
                .next_cursor
                .as_ref()
                .map(encode_run_cursor)
                .transpose()?,
        })
    }

    pub(crate) fn attention_summary(
        &self,
        input: AutomationAttentionSummaryInputDto,
    ) -> Result<AutomationAttentionSummaryOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_limit(input.limit)?;
        let cursor = input
            .cursor
            .as_deref()
            .map(decode_attention_cursor)
            .transpose()?;
        let summary = self
            .storage
            .automation_attention_summary()
            .map_err(AutomationServiceError::internal)?;
        let page = self
            .storage
            .list_automation_attentions(cursor.as_ref(), input.limit as usize)
            .map_err(AutomationServiceError::internal)?;
        Ok(AutomationAttentionSummaryOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            unread_count: summary.total_count,
            items: page.items.iter().map(attention_dto).collect(),
            next_cursor: page
                .next_cursor
                .as_ref()
                .map(encode_attention_cursor)
                .transpose()?,
            last_sequence: nonnegative_u64(summary.last_event_sequence)?,
        })
    }

    pub(crate) fn acknowledge_attention(
        &self,
        input: AutomationAttentionAcknowledgeInputDto,
    ) -> Result<AutomationAttentionAcknowledgeOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.attention_id, "attentionId")?;
        let record = self
            .storage
            .acknowledge_automation_attention_record(&input.attention_id, now_ms())
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| AutomationServiceError::not_found(None))?;
        Ok(AutomationAttentionAcknowledgeOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            attention: attention_dto(&record),
        })
    }

    fn cas_task(
        &self,
        outcome: AutomationCompareAndSetOutcome,
        automation_id: &str,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        match outcome {
            AutomationCompareAndSetOutcome::Updated(record) => self.task_dto(&record),
            AutomationCompareAndSetOutcome::RevisionConflict(record) => {
                Err(AutomationServiceError::revision_conflict(&record))
            }
            AutomationCompareAndSetOutcome::NotFound => Err(AutomationServiceError::not_found(
                Some(automation_id.to_string()),
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_config(
        &self,
        title: String,
        prompt: String,
        destination: AutomationDestinationInputDto,
        permission_mode: AutomationPermissionModeDto,
        permission_mode_version: u32,
        schedule: mycopilot_protocol_rs::AutomationScheduleInputDto,
        notification_policy: AutomationNotificationPolicyDto,
        status: StoredAutomationStatus,
    ) -> Result<AutomationConfigRecord, AutomationServiceError> {
        let preferences = self
            .storage
            .load_ui_preferences()
            .map_err(AutomationServiceError::internal)?;
        let permissions =
            resolve_automation_permissions(permission_mode, permission_mode_version, &preferences)
                .map_err(|error| {
                    if error.code() == "permission_disabled" {
                        AutomationServiceError::permission_disabled(error.to_string())
                    } else {
                        AutomationServiceError::validation(
                            error.to_string(),
                            Some("permissionModeVersion"),
                        )
                    }
                })?;
        let normalized = normalize_schedule(&schedule).map_err(AutomationServiceError::schedule)?;
        let next_run_at = if status == StoredAutomationStatus::Active {
            Some(
                normalized
                    .next_run_at_ms(now_ms())
                    .map_err(AutomationServiceError::schedule)?,
            )
        } else {
            None
        };
        let projects = self
            .storage
            .load_projects()
            .map_err(AutomationServiceError::internal)?;
        let models = self
            .storage
            .load_model_settings_catalog()
            .map_err(AutomationServiceError::internal)?
            .map(|settings| settings.models)
            .unwrap_or_default();
        let target = resolve_target(destination, &projects, &models, self.storage)?;
        let schedule_json = serde_json::to_string(&normalized.schedule)
            .map_err(|_| AutomationServiceError::internal("Schedule serialization failed"))?;
        let permissions_json = serde_json::to_string(&permissions.projection)
            .map_err(|_| AutomationServiceError::internal("Permission serialization failed"))?;
        Ok(AutomationConfigRecord {
            title: title.trim().to_string(),
            prompt: prompt.trim().to_string(),
            health_state: "ok".to_string(),
            blocked_code: None,
            blocked_message: None,
            destination_kind: target.destination_kind,
            target_conversation_id: target.target_conversation_id,
            project_binding_kind: target.project_binding_kind,
            project_id: target.project_id,
            model_id: target.model_id,
            permission_mode: permission_mode_string(permissions.mode).to_string(),
            permission_mode_version: i64::from(permissions.mode_version),
            permissions_json,
            reasoning_json: target.reasoning_json,
            schedule_kind: schedule_kind(&normalized.schedule).to_string(),
            schedule_json,
            rrule: normalized.rrule,
            timezone: normalized.timezone.to_string(),
            anchor_at: normalized.anchor_at,
            next_run_at,
            notification_policy: notification_policy_string(notification_policy).to_string(),
            target_project_snapshot: target.target_project_snapshot,
            target_conversation_snapshot: target.target_conversation_snapshot,
            target_model_snapshot: target.target_model_snapshot,
            target_project_id_snapshot: target.target_project_id_snapshot,
            target_conversation_id_snapshot: target.target_conversation_id_snapshot,
            target_model_id_snapshot: target.target_model_id_snapshot,
        })
    }

    fn task_dto(
        &self,
        record: &AutomationRecord,
    ) -> Result<AutomationTaskDto, AutomationServiceError> {
        let latest_run = self
            .storage
            .get_latest_automation_run(&record.id)
            .map_err(AutomationServiceError::internal)?
            .as_ref()
            .map(run_dto)
            .transpose()?;
        automation_task_dto(record, latest_run)
    }

    pub(crate) fn current_target_block(
        &self,
        task: &AutomationRecord,
    ) -> Result<Option<TargetBlock>, AutomationServiceError> {
        let projects = self
            .storage
            .load_projects()
            .map_err(AutomationServiceError::internal)?;
        let models = self
            .storage
            .load_model_settings_catalog()
            .map_err(AutomationServiceError::internal)?
            .map(|settings| settings.models)
            .unwrap_or_default();

        if task.config.destination_kind == "new_chat" {
            if task.config.project_binding_kind == "project" {
                let project_id = task
                    .config
                    .project_id
                    .as_ref()
                    .or(task.config.target_project_id_snapshot.as_ref());
                let Some(project) = project_id
                    .and_then(|id| projects.iter().find(|project| project.id == id.as_str()))
                else {
                    return Ok(Some(TargetBlock::PROJECT_MISSING));
                };
                if project
                    .primary_path()
                    .is_none_or(|path| !Path::new(path).is_dir())
                {
                    return Ok(Some(TargetBlock::PROJECT_PATH_MISSING));
                }
            }
            let model_id = task
                .config
                .model_id
                .as_ref()
                .or(task.config.target_model_id_snapshot.as_ref());
            let Some(model) =
                model_id.and_then(|id| models.iter().find(|model| model.id == id.as_str()))
            else {
                return Ok(Some(TargetBlock::MODEL_MISSING));
            };
            return Ok((!model.enabled).then_some(TargetBlock::MODEL_DISABLED));
        }

        if task.config.destination_kind != "existing_chat" {
            return Err(AutomationServiceError::internal(
                "Stored destination is invalid",
            ));
        }
        let conversation_id = task
            .config
            .target_conversation_id
            .as_ref()
            .or(task.config.target_conversation_id_snapshot.as_ref());
        let conversations = self
            .storage
            .load_conversation_metas()
            .map_err(AutomationServiceError::internal)?;
        let Some(conversation) = conversation_id.and_then(|id| {
            conversations
                .into_iter()
                .find(|conversation| conversation.id == id.as_str())
        }) else {
            return Ok(Some(TargetBlock::TARGET_MISSING));
        };
        if conversation.archived_at.is_some() {
            return Ok(Some(TargetBlock::TARGET_ARCHIVED));
        }
        if let Some(project_id) = conversation.project_id.as_deref() {
            let Some(project) = projects.iter().find(|project| project.id == project_id) else {
                return Ok(Some(TargetBlock::PROJECT_MISSING));
            };
            if project
                .primary_path()
                .is_none_or(|path| !Path::new(path).is_dir())
            {
                return Ok(Some(TargetBlock::PROJECT_PATH_MISSING));
            }
        }
        if let Some(model_id) = conversation.model_id.as_deref() {
            let Some(model) = models.iter().find(|model| model.id == model_id) else {
                return Ok(Some(TargetBlock::MODEL_MISSING));
            };
            if !model.enabled {
                return Ok(Some(TargetBlock::MODEL_DISABLED));
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TargetBlock {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl TargetBlock {
    const TARGET_MISSING: Self = Self {
        code: "target_missing",
        message: "The selected conversation no longer exists.",
    };
    const TARGET_ARCHIVED: Self = Self {
        code: "target_archived",
        message: "The selected conversation is archived.",
    };
    const PROJECT_MISSING: Self = Self {
        code: "project_missing",
        message: "The selected project no longer exists.",
    };
    const PROJECT_PATH_MISSING: Self = Self {
        code: "project_path_missing",
        message: "The selected project path is not available.",
    };
    const MODEL_MISSING: Self = Self {
        code: "model_missing",
        message: "The selected model no longer exists.",
    };
    const MODEL_DISABLED: Self = Self {
        code: "model_disabled",
        message: "The selected model is disabled.",
    };
}

struct ResolvedTarget {
    destination_kind: String,
    target_conversation_id: Option<String>,
    project_binding_kind: String,
    project_id: Option<String>,
    model_id: Option<String>,
    reasoning_json: Option<String>,
    target_project_snapshot: Option<String>,
    target_conversation_snapshot: Option<String>,
    target_model_snapshot: Option<String>,
    target_project_id_snapshot: Option<String>,
    target_conversation_id_snapshot: Option<String>,
    target_model_id_snapshot: Option<String>,
}

fn resolve_target(
    destination: AutomationDestinationInputDto,
    projects: &[ProjectRecord],
    models: &[ModelConfigRecord],
    storage: &StorageService,
) -> Result<ResolvedTarget, AutomationServiceError> {
    match destination {
        AutomationDestinationInputDto::NewChat {
            project_binding,
            project_id,
            model_id,
        } => {
            let project = match project_binding {
                AutomationProjectBindingDto::None => {
                    if project_id.is_some() {
                        return Err(AutomationServiceError::target_invalid(
                            None,
                            "projectId must be null when projectBinding is none",
                        ));
                    }
                    None
                }
                AutomationProjectBindingDto::Project => {
                    let id = project_id.as_deref().ok_or_else(|| {
                        AutomationServiceError::target_invalid(
                            None,
                            "projectId is required when projectBinding is project",
                        )
                    })?;
                    let project = projects
                        .iter()
                        .find(|project| project.id == id)
                        .ok_or_else(|| {
                            AutomationServiceError::target_invalid(
                                None,
                                "The selected project does not exist.",
                            )
                        })?;
                    if project
                        .primary_path()
                        .is_none_or(|path| !Path::new(path).is_dir())
                    {
                        return Err(AutomationServiceError::target_invalid(
                            None,
                            "The selected project path is not available.",
                        ));
                    }
                    Some(project)
                }
            };
            let model = models
                .iter()
                .find(|model| model.id == model_id)
                .ok_or_else(|| {
                    AutomationServiceError::target_invalid(
                        None,
                        "The selected model does not exist.",
                    )
                })?;
            if !model.enabled {
                return Err(AutomationServiceError::target_invalid(
                    None,
                    "The selected model is disabled.",
                ));
            }
            let reasoning = reasoning_projection(model);
            Ok(ResolvedTarget {
                destination_kind: "new_chat".to_string(),
                target_conversation_id: None,
                project_binding_kind: match project_binding {
                    AutomationProjectBindingDto::None => "none",
                    AutomationProjectBindingDto::Project => "project",
                }
                .to_string(),
                project_id: project.map(|project| project.id.clone()),
                model_id: Some(model.id.clone()),
                reasoning_json: Some(serde_json::to_string(&reasoning).map_err(|_| {
                    AutomationServiceError::internal("Reasoning serialization failed")
                })?),
                target_project_snapshot: project.map(|project| project.name.clone()),
                target_conversation_snapshot: None,
                target_model_snapshot: Some(model.display_label()),
                target_project_id_snapshot: project.map(|project| project.id.clone()),
                target_conversation_id_snapshot: None,
                target_model_id_snapshot: Some(model.id.clone()),
            })
        }
        AutomationDestinationInputDto::ExistingChat { conversation_id } => {
            let conversation = storage
                .load_conversation_metas()
                .map_err(AutomationServiceError::internal)?
                .into_iter()
                .find(|conversation| conversation.id == conversation_id)
                .ok_or_else(|| {
                    AutomationServiceError::target_invalid(
                        None,
                        "The selected root conversation does not exist.",
                    )
                })?;
            if conversation.archived_at.is_some() {
                return Err(AutomationServiceError::target_invalid(
                    None,
                    "The selected conversation is archived.",
                ));
            }
            if let Some(agent) = storage
                .get_agent_node_by_conversation(&conversation_id)
                .map_err(|error| AutomationServiceError::internal(error.to_string()))?
            {
                if agent.parent_agent_id.is_some()
                    || agent.lifecycle != mycopilot_core::AgentLifecycle::Active
                {
                    return Err(AutomationServiceError::target_invalid(
                        None,
                        "The selected conversation is not an active root chat.",
                    ));
                }
            }
            let project = match conversation.project_id.as_deref() {
                Some(id) => {
                    let project = projects
                        .iter()
                        .find(|project| project.id == id)
                        .ok_or_else(|| {
                            AutomationServiceError::target_invalid(
                                None,
                                "The conversation project does not exist.",
                            )
                        })?;
                    if project
                        .primary_path()
                        .is_none_or(|path| !Path::new(path).is_dir())
                    {
                        return Err(AutomationServiceError::target_invalid(
                            None,
                            "The conversation project path is not available.",
                        ));
                    }
                    Some(project)
                }
                None => None,
            };
            let model = match conversation.model_id.as_deref() {
                Some(id) => {
                    let model = models.iter().find(|model| model.id == id).ok_or_else(|| {
                        AutomationServiceError::target_invalid(
                            None,
                            "The conversation model does not exist.",
                        )
                    })?;
                    if !model.enabled {
                        return Err(AutomationServiceError::target_invalid(
                            None,
                            "The conversation model is disabled.",
                        ));
                    }
                    Some(model)
                }
                None => None,
            };
            Ok(ResolvedTarget {
                destination_kind: "existing_chat".to_string(),
                target_conversation_id: Some(conversation.id.clone()),
                project_binding_kind: "inherit".to_string(),
                project_id: None,
                model_id: None,
                reasoning_json: None,
                target_project_snapshot: project.map(|value| value.name.clone()),
                target_conversation_snapshot: Some(conversation.title.clone()),
                target_model_snapshot: model.map(|value| value.display_label()),
                target_project_id_snapshot: conversation.project_id.clone(),
                target_conversation_id_snapshot: Some(conversation.id),
                target_model_id_snapshot: conversation.model_id,
            })
        }
    }
}

fn automation_task_dto(
    record: &AutomationRecord,
    latest_run: Option<AutomationRunDto>,
) -> Result<AutomationTaskDto, AutomationServiceError> {
    let schedule = parse_schedule(&record.config.schedule_json)?;
    let schedule_summary = normalize_schedule(&schedule)
        .map_err(AutomationServiceError::schedule)?
        .summary();
    let permissions = serde_json::from_str(&record.config.permissions_json)
        .map_err(|_| AutomationServiceError::internal("Stored permissions are invalid"))?;
    let destination = if record.config.destination_kind == "new_chat" {
        let reasoning = record
            .config
            .reasoning_json
            .as_deref()
            .ok_or_else(|| AutomationServiceError::internal("Stored reasoning is missing"))
            .and_then(|value| {
                serde_json::from_str(value)
                    .map_err(|_| AutomationServiceError::internal("Stored reasoning is invalid"))
            })?;
        let model_id = retained_id(
            record.config.model_id.as_ref(),
            record.config.target_model_id_snapshot.as_ref(),
            "model",
        )?;
        let project_id = record
            .config
            .project_id
            .clone()
            .or_else(|| record.config.target_project_id_snapshot.clone());
        AutomationDestinationDto::NewChat {
            project_binding: match record.config.project_binding_kind.as_str() {
                "none" => AutomationProjectBindingDto::None,
                "project" => AutomationProjectBindingDto::Project,
                _ => {
                    return Err(AutomationServiceError::internal(
                        "Stored project binding is invalid",
                    ))
                }
            },
            project_id,
            model_id,
            reasoning,
        }
    } else if record.config.destination_kind == "existing_chat" {
        AutomationDestinationDto::ExistingChat {
            conversation_id: retained_id(
                record.config.target_conversation_id.as_ref(),
                record.config.target_conversation_id_snapshot.as_ref(),
                "conversation",
            )?,
        }
    } else {
        return Err(AutomationServiceError::internal(
            "Stored destination is invalid",
        ));
    };
    let task_attention = task_attention(record);
    Ok(AutomationTaskDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        automation_id: record.id.clone(),
        title: record.config.title.clone(),
        prompt: record.config.prompt.clone(),
        status: protocol_status(record.status),
        health: health_dto(record)?,
        destination,
        permission_mode: parse_permission_mode(&record.config.permission_mode)?,
        permission_mode_version: u32::try_from(record.config.permission_mode_version).map_err(
            |_| AutomationServiceError::internal("Stored permission version is invalid"),
        )?,
        resolved_permissions: permissions,
        schedule,
        schedule_summary,
        rrule: record.config.rrule.clone(),
        timezone: record.config.timezone.clone(),
        notification_policy: parse_notification_policy(&record.config.notification_policy)?,
        target_snapshot: AutomationTargetSnapshotDto {
            project_name: record.config.target_project_snapshot.clone(),
            conversation_title: record.config.target_conversation_snapshot.clone(),
            model_display_name: record.config.target_model_snapshot.clone(),
        },
        next_run_at: record.config.next_run_at,
        last_scheduled_at: record.last_scheduled_at,
        last_run_at: record.last_run_at,
        latest_run,
        attention: task_attention,
        revision: nonnegative_u64(record.revision)?,
        created_at: record.created_at,
        updated_at: record.updated_at,
    })
}

pub(crate) fn run_dto(
    record: &AutomationRunRecord,
) -> Result<AutomationRunDto, AutomationServiceError> {
    Ok(AutomationRunDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        run_id: record.id.clone(),
        automation_id: record.automation_id.clone(),
        config_revision: nonnegative_u64(record.config_revision)?,
        trigger_kind: match record.trigger_kind.as_str() {
            "scheduled" => AutomationRunTriggerKindDto::Scheduled,
            "manual" => AutomationRunTriggerKindDto::Manual,
            "recovery" => AutomationRunTriggerKindDto::Recovery,
            _ => {
                return Err(AutomationServiceError::internal(
                    "Stored run trigger is invalid",
                ))
            }
        },
        scheduled_for: Some(record.scheduled_for),
        status: match record.status {
            StoredAutomationRunStatus::Queued => AutomationRunStatusDto::Queued,
            StoredAutomationRunStatus::Admitting => AutomationRunStatusDto::Starting,
            StoredAutomationRunStatus::Running => AutomationRunStatusDto::Running,
            StoredAutomationRunStatus::WaitingForApproval => {
                AutomationRunStatusDto::WaitingForApproval
            }
            StoredAutomationRunStatus::Completed => AutomationRunStatusDto::Completed,
            StoredAutomationRunStatus::Failed => AutomationRunStatusDto::Failed,
            StoredAutomationRunStatus::Cancelled => AutomationRunStatusDto::Cancelled,
        },
        conversation_id: record.conversation_id.clone(),
        user_message_id: record.user_message_id.clone(),
        assistant_message_id: record.assistant_message_id.clone(),
        report_kind: parse_report_kind(record.report_kind.as_deref())?,
        result_preview: record.result_preview.clone(),
        error_code: record.error_code.clone(),
        error_message: record.error_message.clone(),
        attention: run_attention(record),
        created_at: record.created_at,
        started_at: record.started_at,
        completed_at: record.completed_at,
        updated_at: record.updated_at,
    })
}

fn task_attention(record: &AutomationRecord) -> Option<AutomationAttentionDto> {
    let required = unread_required_at(record.attention_required_at, record.attention_read_at)?;
    Some(AutomationAttentionDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        attention_id: format!("task:{}", record.id),
        automation_id: record.id.clone(),
        run_id: None,
        kind: AutomationAttentionKindDto::ConfigurationBlocked,
        message: record
            .config
            .blocked_message
            .clone()
            .unwrap_or_else(|| "The automation configuration needs attention.".to_string()),
        created_at: required,
        read_at: record.attention_read_at,
    })
}

fn run_attention(record: &AutomationRunRecord) -> Option<AutomationAttentionDto> {
    let required = unread_required_at(record.attention_required_at, record.attention_read_at)?;
    let kind = match record.status {
        StoredAutomationRunStatus::WaitingForApproval => {
            AutomationAttentionKindDto::WaitingForApproval
        }
        StoredAutomationRunStatus::Failed | StoredAutomationRunStatus::Cancelled => {
            AutomationAttentionKindDto::RunFailed
        }
        _ => AutomationAttentionKindDto::ImportantUpdate,
    };
    Some(AutomationAttentionDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        attention_id: format!("run:{}", record.id),
        automation_id: record.automation_id.clone(),
        run_id: Some(record.id.clone()),
        kind,
        message: record
            .error_message
            .clone()
            .or_else(|| record.result_preview.clone())
            .unwrap_or_else(|| "This automation run needs attention.".to_string()),
        created_at: required,
        read_at: record.attention_read_at,
    })
}

fn attention_dto(record: &AutomationAttentionRecord) -> AutomationAttentionDto {
    let kind = match record.attention_kind.as_str() {
        "configuration_blocked" => AutomationAttentionKindDto::ConfigurationBlocked,
        "waiting_for_approval" => AutomationAttentionKindDto::WaitingForApproval,
        "run_failed" => AutomationAttentionKindDto::RunFailed,
        "important_update" => AutomationAttentionKindDto::ImportantUpdate,
        _ => AutomationAttentionKindDto::ImportantUpdate,
    };
    AutomationAttentionDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        attention_id: record.attention_id.clone(),
        automation_id: record.automation_id.clone(),
        run_id: record.automation_run_id.clone(),
        kind,
        message: record
            .message
            .clone()
            .unwrap_or_else(|| "This automation needs attention.".to_string()),
        created_at: record.required_at,
        read_at: record.read_at,
    }
}

fn unread_required_at(required: Option<i64>, read: Option<i64>) -> Option<i64> {
    required.filter(|required| read.is_none_or(|read| read < *required))
}

pub(crate) fn automation_event_dto(
    record: AutomationEventRecord,
) -> Result<AutomationEventDto, AutomationServiceError> {
    Ok(AutomationEventDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        sequence: nonnegative_u64(record.sequence)?,
        event_id: record.event_id,
        kind: match record.event_kind.as_str() {
            "created" => AutomationEventKindDto::Created,
            "updated" => AutomationEventKindDto::Updated,
            "deleted" => AutomationEventKindDto::Deleted,
            "run_updated" => AutomationEventKindDto::RunUpdated,
            "attention_changed" => AutomationEventKindDto::AttentionChanged,
            "notification_requested" => AutomationEventKindDto::NotificationRequested,
            _ => {
                return Err(AutomationServiceError::internal(
                    "Stored event kind is invalid",
                ))
            }
        },
        automation_id: record.automation_id,
        run_id: record.automation_run_id,
        resource_revision: record.resource_revision.map(nonnegative_u64).transpose()?,
        occurred_at: record.occurred_at,
    })
}

fn health_dto(record: &AutomationRecord) -> Result<AutomationHealthDto, AutomationServiceError> {
    match record.config.health_state.as_str() {
        "ok" => Ok(AutomationHealthDto::Ok),
        "blocked" => Ok(AutomationHealthDto::Blocked {
            code: match record.config.blocked_code.as_deref() {
                Some("target_missing") => AutomationBlockedCodeDto::TargetMissing,
                Some("target_archived") => AutomationBlockedCodeDto::TargetArchived,
                Some("target_invalid") => AutomationBlockedCodeDto::TargetInvalid,
                Some("project_missing") => AutomationBlockedCodeDto::ProjectMissing,
                Some("project_path_missing") => AutomationBlockedCodeDto::ProjectPathMissing,
                Some("model_missing") => AutomationBlockedCodeDto::ModelMissing,
                Some("model_disabled") => AutomationBlockedCodeDto::ModelDisabled,
                Some("permission_disabled") => AutomationBlockedCodeDto::PermissionDisabled,
                Some("configuration_invalid") => AutomationBlockedCodeDto::ConfigurationInvalid,
                Some("schedule_invalid") => AutomationBlockedCodeDto::ScheduleInvalid,
                _ => {
                    return Err(AutomationServiceError::internal(
                        "Stored blocked code is invalid",
                    ))
                }
            },
            message: record.config.blocked_message.clone().ok_or_else(|| {
                AutomationServiceError::internal("Stored blocked message is missing")
            })?,
        }),
        _ => Err(AutomationServiceError::internal(
            "Stored health state is invalid",
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CursorEnvelope {
    Task {
        updated_at: i64,
        id: String,
    },
    Run {
        created_at: i64,
        id: String,
    },
    Attention {
        required_at: i64,
        attention_id: String,
    },
}

fn encode_cursor(cursor: &CursorEnvelope) -> Result<String, AutomationServiceError> {
    serde_json::to_vec(cursor)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| AutomationServiceError::internal("Cursor serialization failed"))
}
fn decode_cursor(value: &str) -> Result<CursorEnvelope, AutomationServiceError> {
    validate_bounded(value, 2_048, false, "cursor")?;
    let bytes = URL_SAFE_NO_PAD.decode(value).map_err(|_| {
        AutomationServiceError::validation("The cursor is invalid.", Some("cursor"))
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AutomationServiceError::validation("The cursor is invalid.", Some("cursor")))
}
fn encode_task_cursor(value: &AutomationListCursor) -> Result<String, AutomationServiceError> {
    encode_cursor(&CursorEnvelope::Task {
        updated_at: value.updated_at,
        id: value.id.clone(),
    })
}
fn decode_task_cursor(value: &str) -> Result<AutomationListCursor, AutomationServiceError> {
    match decode_cursor(value)? {
        CursorEnvelope::Task { updated_at, id } => Ok(AutomationListCursor { updated_at, id }),
        _ => Err(AutomationServiceError::validation(
            "The task cursor has the wrong type.",
            Some("cursor"),
        )),
    }
}
fn encode_run_cursor(value: &AutomationRunListCursor) -> Result<String, AutomationServiceError> {
    encode_cursor(&CursorEnvelope::Run {
        created_at: value.created_at,
        id: value.id.clone(),
    })
}
fn decode_run_cursor(value: &str) -> Result<AutomationRunListCursor, AutomationServiceError> {
    match decode_cursor(value)? {
        CursorEnvelope::Run { created_at, id } => Ok(AutomationRunListCursor { created_at, id }),
        _ => Err(AutomationServiceError::validation(
            "The run cursor has the wrong type.",
            Some("cursor"),
        )),
    }
}
fn encode_attention_cursor(
    value: &AutomationAttentionListCursor,
) -> Result<String, AutomationServiceError> {
    encode_cursor(&CursorEnvelope::Attention {
        required_at: value.required_at,
        attention_id: value.attention_id.clone(),
    })
}
fn decode_attention_cursor(
    value: &str,
) -> Result<AutomationAttentionListCursor, AutomationServiceError> {
    match decode_cursor(value)? {
        CursorEnvelope::Attention {
            required_at,
            attention_id,
        } => Ok(AutomationAttentionListCursor {
            required_at,
            attention_id,
        }),
        _ => Err(AutomationServiceError::validation(
            "The attention cursor has the wrong type.",
            Some("cursor"),
        )),
    }
}

pub(crate) fn config_snapshot(task: &AutomationRecord) -> Result<String, AutomationServiceError> {
    let permissions: mycopilot_protocol_rs::AutomationResolvedPermissionsDto =
        serde_json::from_str(&task.config.permissions_json)
            .map_err(|_| AutomationServiceError::internal("Stored permissions are invalid"))?;
    let reasoning: Option<mycopilot_protocol_rs::AutomationReasoningProjectionDto> = task
        .config
        .reasoning_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| AutomationServiceError::internal("Stored reasoning is invalid"))?;
    let schedule: mycopilot_protocol_rs::AutomationScheduleDto =
        serde_json::from_str(&task.config.schedule_json)
            .map_err(|_| AutomationServiceError::internal("Stored schedule is invalid"))?;
    serde_json::to_string(&AutomationConfigSnapshot {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        automation_id: task.id.clone(),
        config_revision: task.revision,
        title: task.config.title.clone(),
        prompt: task.config.prompt.clone(),
        destination_kind: task.config.destination_kind.clone(),
        target_conversation_id: task.config.target_conversation_id.clone(),
        project_binding_kind: task.config.project_binding_kind.clone(),
        project_id: task.config.project_id.clone(),
        model_id: task.config.model_id.clone(),
        permission_mode: task.config.permission_mode.clone(),
        permission_mode_version: task.config.permission_mode_version,
        permissions,
        reasoning,
        schedule,
        rrule: task.config.rrule.clone(),
        timezone: task.config.timezone.clone(),
        notification_policy: task.config.notification_policy.clone(),
    })
    .map_err(|_| AutomationServiceError::internal("Configuration snapshot serialization failed"))
}

pub(crate) fn validate_schema(version: u32) -> Result<(), AutomationServiceError> {
    if version == AUTOMATION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(AutomationServiceError::validation(
            "Unsupported automation schema version.",
            Some("schemaVersion"),
        ))
    }
}
fn validate_limit(limit: u32) -> Result<(), AutomationServiceError> {
    if (1..=MAX_PAGE_SIZE).contains(&limit) {
        Ok(())
    } else {
        Err(AutomationServiceError::validation(
            "limit must be between 1 and 100.",
            Some("limit"),
        ))
    }
}
pub(crate) fn validate_id(value: &str, field: &'static str) -> Result<(), AutomationServiceError> {
    validate_bounded(value, MAX_ID_BYTES, false, field)
}
fn validate_title_prompt(title: &str, prompt: &str) -> Result<(), AutomationServiceError> {
    validate_bounded(title, MAX_TITLE_BYTES, false, "title")?;
    if prompt.trim().is_empty()
        || prompt.len() > MAX_PROMPT_BYTES
        || prompt
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        Err(AutomationServiceError::validation(
            "prompt is invalid.",
            Some("prompt"),
        ))
    } else {
        Ok(())
    }
}
fn validate_bounded(
    value: &str,
    max: usize,
    allow_empty: bool,
    field: &'static str,
) -> Result<(), AutomationServiceError> {
    if (!allow_empty && value.trim().is_empty())
        || value.len() > max
        || value.chars().any(char::is_control)
    {
        Err(AutomationServiceError::validation(
            format!("{field} is invalid."),
            Some(field),
        ))
    } else {
        Ok(())
    }
}
fn validate_notification_policy(
    destination: &AutomationDestinationInputDto,
    policy: AutomationNotificationPolicyDto,
) -> Result<(), AutomationServiceError> {
    let valid = match destination {
        AutomationDestinationInputDto::NewChat { .. } => matches!(
            policy,
            AutomationNotificationPolicyDto::AllRuns
                | AutomationNotificationPolicyDto::UnsuccessfulOnly
        ),
        AutomationDestinationInputDto::ExistingChat { .. } => matches!(
            policy,
            AutomationNotificationPolicyDto::ImportantUpdates
                | AutomationNotificationPolicyDto::UnsuccessfulOnly
        ),
    };
    if valid {
        Ok(())
    } else {
        Err(AutomationServiceError::validation(
            "The notification policy is not valid for this destination.",
            Some("notificationPolicy"),
        ))
    }
}
fn parse_schedule(
    value: &str,
) -> Result<mycopilot_protocol_rs::AutomationScheduleDto, AutomationServiceError> {
    serde_json::from_str(value)
        .map_err(|_| AutomationServiceError::internal("Stored schedule is invalid"))
}
fn schedule_kind(value: &mycopilot_protocol_rs::AutomationScheduleDto) -> &'static str {
    match value {
        mycopilot_protocol_rs::AutomationScheduleInputDto::Interval { .. } => "interval",
        mycopilot_protocol_rs::AutomationScheduleInputDto::Daily { .. } => "daily",
        mycopilot_protocol_rs::AutomationScheduleInputDto::Weekdays { .. } => "weekdays",
        mycopilot_protocol_rs::AutomationScheduleInputDto::Weekly { .. } => "weekly",
        mycopilot_protocol_rs::AutomationScheduleInputDto::Custom { .. } => "custom",
    }
}
fn stored_status(value: AutomationStatusDto) -> StoredAutomationStatus {
    match value {
        AutomationStatusDto::Active => StoredAutomationStatus::Active,
        AutomationStatusDto::Paused => StoredAutomationStatus::Paused,
    }
}
fn protocol_status(value: StoredAutomationStatus) -> AutomationStatusDto {
    match value {
        StoredAutomationStatus::Active => AutomationStatusDto::Active,
        StoredAutomationStatus::Paused => AutomationStatusDto::Paused,
    }
}
fn permission_mode_string(value: AutomationPermissionModeDto) -> &'static str {
    match value {
        AutomationPermissionModeDto::Default => "default",
        AutomationPermissionModeDto::Full => "full",
        AutomationPermissionModeDto::Custom => "custom",
    }
}
pub(crate) fn parse_permission_mode(
    value: &str,
) -> Result<AutomationPermissionModeDto, AutomationServiceError> {
    match value {
        "default" => Ok(AutomationPermissionModeDto::Default),
        "full" => Ok(AutomationPermissionModeDto::Full),
        "custom" => Ok(AutomationPermissionModeDto::Custom),
        _ => Err(AutomationServiceError::internal(
            "Stored permission mode is invalid",
        )),
    }
}
fn notification_policy_string(value: AutomationNotificationPolicyDto) -> &'static str {
    match value {
        AutomationNotificationPolicyDto::AllRuns => "all_runs",
        AutomationNotificationPolicyDto::UnsuccessfulOnly => "unsuccessful_only",
        AutomationNotificationPolicyDto::ImportantUpdates => "important_updates",
    }
}
fn parse_notification_policy(
    value: &str,
) -> Result<AutomationNotificationPolicyDto, AutomationServiceError> {
    match value {
        "all_runs" => Ok(AutomationNotificationPolicyDto::AllRuns),
        "unsuccessful_only" => Ok(AutomationNotificationPolicyDto::UnsuccessfulOnly),
        "important_updates" => Ok(AutomationNotificationPolicyDto::ImportantUpdates),
        _ => Err(AutomationServiceError::internal(
            "Stored notification policy is invalid",
        )),
    }
}
fn parse_report_kind(
    value: Option<&str>,
) -> Result<AutomationReportKindDto, AutomationServiceError> {
    match value.unwrap_or("unknown") {
        "no_change" => Ok(AutomationReportKindDto::NoChange),
        "important_update" => Ok(AutomationReportKindDto::ImportantUpdate),
        "completed" => Ok(AutomationReportKindDto::Completed),
        "unknown" => Ok(AutomationReportKindDto::Unknown),
        _ => Err(AutomationServiceError::internal(
            "Stored report kind is invalid",
        )),
    }
}
fn retained_id(
    current: Option<&String>,
    snapshot: Option<&String>,
    label: &str,
) -> Result<String, AutomationServiceError> {
    current.or(snapshot).cloned().ok_or_else(|| {
        AutomationServiceError::internal(format!("Stored {label} identity is missing"))
    })
}
fn revision_i64(value: u64) -> Result<i64, AutomationServiceError> {
    i64::try_from(value).map_err(|_| {
        AutomationServiceError::validation(
            "expectedRevision is too large.",
            Some("expectedRevision"),
        )
    })
}
fn nonnegative_u64(value: i64) -> Result<u64, AutomationServiceError> {
    u64::try_from(value).map_err(|_| AutomationServiceError::internal("Stored counter is negative"))
}

#[derive(Debug, Clone)]
pub(crate) struct AutomationServiceError {
    kind: AutomationErrorKindDto,
    message: String,
    automation_id: Option<String>,
    current_revision: Option<u64>,
    field: Option<String>,
    retryable: bool,
}

impl AutomationServiceError {
    pub(crate) fn validation(message: impl Into<String>, field: Option<&str>) -> Self {
        Self {
            kind: AutomationErrorKindDto::Validation,
            message: message.into(),
            automation_id: None,
            current_revision: None,
            field: field.map(str::to_string),
            retryable: false,
        }
    }
    fn schedule(error: AutomationScheduleError) -> Self {
        Self {
            kind: AutomationErrorKindDto::ScheduleInvalid,
            message: error.to_string(),
            automation_id: None,
            current_revision: None,
            field: Some(error.field.to_string()),
            retryable: false,
        }
    }
    fn permission_disabled(message: impl Into<String>) -> Self {
        Self {
            kind: AutomationErrorKindDto::PermissionDisabled,
            message: message.into(),
            automation_id: None,
            current_revision: None,
            field: Some("permissionMode".to_string()),
            retryable: false,
        }
    }
    fn target_invalid(id: Option<String>, message: impl Into<String>) -> Self {
        Self {
            kind: AutomationErrorKindDto::TargetInvalid,
            message: message.into(),
            automation_id: id,
            current_revision: None,
            field: Some("destination".to_string()),
            retryable: false,
        }
    }
    fn not_found(id: Option<String>) -> Self {
        Self {
            kind: AutomationErrorKindDto::NotFound,
            message: "The automation resource was not found.".to_string(),
            automation_id: id,
            current_revision: None,
            field: None,
            retryable: false,
        }
    }
    fn revision_conflict(record: &AutomationRecord) -> Self {
        Self {
            kind: AutomationErrorKindDto::RevisionConflict,
            message: "The automation was updated elsewhere.".to_string(),
            automation_id: Some(record.id.clone()),
            current_revision: u64::try_from(record.revision).ok(),
            field: None,
            retryable: true,
        }
    }
    fn active_run(automation_id: &str, _run_id: &str) -> Self {
        Self {
            kind: AutomationErrorKindDto::RunAlreadyActive,
            message: "This automation already has an active run.".to_string(),
            automation_id: Some(automation_id.to_string()),
            current_revision: None,
            field: None,
            retryable: true,
        }
    }
    pub(crate) fn internal(_message: impl Into<String>) -> Self {
        Self {
            kind: AutomationErrorKindDto::Internal,
            message: "The automation operation could not be completed.".to_string(),
            automation_id: None,
            current_revision: None,
            field: None,
            retryable: true,
        }
    }
    pub(crate) fn data(&self) -> AutomationErrorDataDto {
        AutomationErrorDataDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            kind: AutomationErrorTypeDto::Automation,
            code: self.kind,
            message: self.message.clone(),
            automation_id: self.automation_id.clone(),
            current_revision: self.current_revision,
            field: self.field.clone(),
            retryable: self.retryable,
        }
    }
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for AutomationServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for AutomationServiceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_are_typed_and_opaque() {
        let encoded = encode_task_cursor(&AutomationListCursor {
            updated_at: 42,
            id: "automation-1".to_string(),
        })
        .unwrap();
        assert_eq!(decode_task_cursor(&encoded).unwrap().updated_at, 42);
        assert!(decode_run_cursor(&encoded).is_err());
        assert!(decode_task_cursor("not-base64!").is_err());
    }

    #[test]
    fn notification_policy_is_destination_scoped() {
        let new_chat = AutomationDestinationInputDto::NewChat {
            project_binding: AutomationProjectBindingDto::None,
            project_id: None,
            model_id: "model-1".to_string(),
        };
        assert!(
            validate_notification_policy(&new_chat, AutomationNotificationPolicyDto::AllRuns)
                .is_ok()
        );
        assert!(validate_notification_policy(
            &new_chat,
            AutomationNotificationPolicyDto::ImportantUpdates
        )
        .is_err());
    }
}
