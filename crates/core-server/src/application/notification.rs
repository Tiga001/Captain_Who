use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use mycopilot_core::storage::notification_repository::{
    NotificationBatchRecord, NotificationChangeEventRecord, NotificationCounts,
    NotificationEventRecord, NotificationListCursor, NotificationSettingsRecord,
    NotificationSettingsUpdate, NotificationSettingsUpdateOutcome, NotificationSummaryRecord,
};
use mycopilot_core::storage::now_ms;
use mycopilot_core::storage::service::StorageService;
use mycopilot_protocol_rs::*;
use serde::{Deserialize, Serialize};

const MAX_ID_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub(crate) struct NotificationServiceError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) field: Option<&'static str>,
}

impl NotificationServiceError {
    fn validation(message: impl Into<String>, field: &'static str) -> Self {
        Self {
            code: "validation",
            message: message.into(),
            field: Some(field),
        }
    }
    fn conflict() -> Self {
        Self {
            code: "revision_conflict",
            message: "Notification settings changed in another window.".into(),
            field: Some("expectedRevision"),
        }
    }
    fn internal(_message: impl Into<String>) -> Self {
        Self {
            code: "internal",
            // Storage errors can contain database paths, SQL fragments, or bound values. Keep the
            // transport error stable and safe; detailed diagnostics belong in trusted server logs.
            message: "The notification operation could not be completed.".into(),
            field: None,
        }
    }
}

pub(crate) struct NotificationService<'a> {
    storage: &'a StorageService,
}

impl<'a> NotificationService<'a> {
    pub(crate) fn new(storage: &'a StorageService) -> Self {
        Self { storage }
    }

    pub(crate) fn claim(
        &self,
        input: NotificationBatchesClaimInputDto,
    ) -> Result<NotificationBatchesClaimOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.claim_token, "claimToken")?;
        if !(5_000..=300_000).contains(&input.lease_duration_ms) {
            return Err(NotificationServiceError::validation(
                "leaseDurationMs is outside the supported range.",
                "leaseDurationMs",
            ));
        }
        if !(1..=10).contains(&input.limit) {
            return Err(NotificationServiceError::validation(
                "limit must be between 1 and 10.",
                "limit",
            ));
        }
        let now = now_ms();
        let records = self
            .storage
            .claim_pending_notification_batches(
                &input.claim_token,
                now,
                i64::try_from(input.lease_duration_ms).map_err(|_| {
                    NotificationServiceError::validation(
                        "leaseDurationMs is invalid.",
                        "leaseDurationMs",
                    )
                })?,
                input.limit as usize,
            )
            .map_err(NotificationServiceError::internal)?;
        let settings = self
            .storage
            .load_notification_settings()
            .map_err(NotificationServiceError::internal)?;
        let batches = records
            .iter()
            .map(|record| self.batch_dto(record, &settings, now))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NotificationBatchesClaimOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            claim_token: input.claim_token,
            batches,
        })
    }

    pub(crate) fn validate(
        &self,
        input: NotificationBatchValidateInputDto,
    ) -> Result<NotificationBatchValidateOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.batch_id, "batchId")?;
        validate_id(&input.claim_token, "claimToken")?;
        let now = now_ms();
        let record = self
            .storage
            .validate_claimed_notification_batch(&input.batch_id, &input.claim_token, now)
            .map_err(NotificationServiceError::internal)?;
        let settings = self
            .storage
            .load_notification_settings()
            .map_err(NotificationServiceError::internal)?;
        let batch = record
            .as_ref()
            .map(|value| self.batch_dto(value, &settings, now))
            .transpose()?;
        Ok(NotificationBatchValidateOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: input.batch_id,
            batch,
        })
    }

    pub(crate) fn acknowledge(
        &self,
        input: NotificationBatchAcknowledgeInputDto,
    ) -> Result<NotificationBatchAcknowledgeOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.batch_id, "batchId")?;
        validate_id(&input.claim_token, "claimToken")?;
        let disposition = disposition_string(input.disposition);
        let native_priority = priority_string(input.native_priority);
        let sound = sound_string(input.sound_level_played);
        let revision = i64::try_from(input.native_revision).map_err(|_| {
            NotificationServiceError::validation("nativeRevision is invalid.", "nativeRevision")
        })?;
        let at = now_ms();
        let record = self
            .storage
            .acknowledge_notification_batch(
                &input.batch_id,
                &input.claim_token,
                disposition,
                native_priority,
                sound,
                revision,
                at,
            )
            .map_err(NotificationServiceError::internal)?
            .ok_or_else(|| {
                NotificationServiceError::validation(
                    "The notification delivery claim is no longer active.",
                    "claimToken",
                )
            })?;
        Ok(NotificationBatchAcknowledgeOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: record.id,
            status: batch_status(&record.status)?,
            disposition: input.disposition,
            acknowledged_at: at,
        })
    }

    pub(crate) fn release(
        &self,
        input: NotificationBatchReleaseInputDto,
    ) -> Result<NotificationBatchReleaseOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.batch_id, "batchId")?;
        validate_id(&input.claim_token, "claimToken")?;
        if input.retry_at < 0 {
            return Err(NotificationServiceError::validation(
                "retryAt must be non-negative.",
                "retryAt",
            ));
        }
        let record = self
            .storage
            .release_notification_batch(
                &input.batch_id,
                &input.claim_token,
                input.retry_at,
                "native_notification_failed",
            )
            .map_err(NotificationServiceError::internal)?
            .ok_or_else(|| {
                NotificationServiceError::validation(
                    "The notification delivery claim is no longer active.",
                    "claimToken",
                )
            })?;
        Ok(NotificationBatchReleaseOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: record.id,
            status: batch_status(&record.status)?,
            retry_at: record.retry_at,
        })
    }

    pub(crate) fn suppress(
        &self,
        input: NotificationBatchSuppressInputDto,
    ) -> Result<NotificationBatchSuppressOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.batch_id, "batchId")?;
        if input.reason == NotificationDeliveryDispositionDto::Delivered {
            return Err(NotificationServiceError::validation(
                "A delivered batch cannot be suppressed.",
                "reason",
            ));
        }
        let at = now_ms();
        let record = self
            .storage
            .suppress_notification_batch(&input.batch_id, disposition_string(input.reason), at)
            .map_err(NotificationServiceError::internal)?
            .ok_or_else(|| {
                NotificationServiceError::validation(
                    "The notification batch is no longer suppressible.",
                    "batchId",
                )
            })?;
        Ok(NotificationBatchSuppressOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: record.id,
            status: NotificationBatchStatusDto::Suppressed,
            reason: input.reason,
            suppressed_at: record.suppressed_at.unwrap_or(at),
        })
    }

    pub(crate) fn list(
        &self,
        input: NotificationListInputDto,
    ) -> Result<NotificationListOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        if !(1..=100).contains(&input.limit) {
            return Err(NotificationServiceError::validation(
                "limit must be between 1 and 100.",
                "limit",
            ));
        }
        if let Some(id) = input.batch_id.as_deref() {
            validate_id(id, "batchId")?;
        }
        let cursor = input.cursor.as_deref().map(decode_cursor).transpose()?;
        let page = self
            .storage
            .list_notifications(
                cursor.as_ref(),
                input.limit as usize,
                input.unread_only,
                input.batch_id.as_deref(),
            )
            .map_err(NotificationServiceError::internal)?;
        Ok(NotificationListOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            items: page
                .items
                .iter()
                .map(item_dto)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor: page.next_cursor.as_ref().map(encode_cursor).transpose()?,
            unread_count: page.unread_count,
            last_sequence: nonnegative(page.last_sequence)?,
        })
    }

    pub(crate) fn summary(
        &self,
        input: NotificationSummaryInputDto,
    ) -> Result<NotificationSummaryOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        summary_dto(
            &self
                .storage
                .notification_summary()
                .map_err(NotificationServiceError::internal)?,
        )
    }

    pub(crate) fn mark_seen(
        &self,
        input: NotificationMarkSeenInputDto,
    ) -> Result<NotificationMarkSeenOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        let at = now_ms();
        let (ids, batch, all) = match input.target {
            NotificationMarkSeenTargetDto::Events { event_ids } => {
                if event_ids.is_empty() || event_ids.len() > 100 {
                    return Err(NotificationServiceError::validation(
                        "eventIds must contain 1 to 100 ids.",
                        "target.eventIds",
                    ));
                }
                for id in &event_ids {
                    validate_id(id, "target.eventIds")?;
                }
                if event_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != event_ids.len()
                {
                    return Err(NotificationServiceError::validation(
                        "eventIds must contain unique ids.",
                        "target.eventIds",
                    ));
                }
                (Some(event_ids), None, false)
            }
            NotificationMarkSeenTargetDto::Batch { batch_id } => {
                validate_id(&batch_id, "target.batchId")?;
                (None, Some(batch_id), false)
            }
            NotificationMarkSeenTargetDto::All => (None, None, true),
        };
        let count = self
            .storage
            .mark_notification_events_seen(ids.as_deref(), batch.as_deref(), all, at)
            .map_err(NotificationServiceError::internal)?;
        let summary = summary_dto(
            &self
                .storage
                .notification_summary()
                .map_err(NotificationServiceError::internal)?,
        )?;
        Ok(NotificationMarkSeenOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            updated_count: count as u64,
            seen_at: at,
            summary,
        })
    }

    pub(crate) fn get_settings(
        &self,
        input: NotificationSettingsGetInputDto,
    ) -> Result<NotificationSettingsGetOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        let value = self
            .storage
            .load_notification_settings()
            .map_err(NotificationServiceError::internal)?;
        Ok(NotificationSettingsGetOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            settings: settings_dto(&value)?,
        })
    }

    pub(crate) fn update_settings(
        &self,
        input: NotificationSettingsUpdateInputDto,
    ) -> Result<NotificationSettingsUpdateOutputDto, NotificationServiceError> {
        validate_schema(input.schema_version)?;
        let expected_revision = i64::try_from(input.expected_revision).map_err(|_| {
            NotificationServiceError::validation("expectedRevision is invalid.", "expectedRevision")
        })?;
        let value = NotificationSettingsUpdate {
            enabled: input.settings.enabled,
            sound_enabled: input.settings.sound_enabled,
            show_task_content: input.settings.show_task_content,
            human_completed_enabled: input.settings.human_completed_enabled,
            human_failed_enabled: input.settings.human_failed_enabled,
            human_approval_enabled: input.settings.human_approval_enabled,
            human_cancelled_enabled: input.settings.human_cancelled_enabled,
            expected_revision,
            updated_at: now_ms(),
        };
        let record = match self
            .storage
            .update_notification_settings(&value)
            .map_err(NotificationServiceError::internal)?
        {
            NotificationSettingsUpdateOutcome::Updated(record) => record,
            NotificationSettingsUpdateOutcome::RevisionConflict(_) => {
                return Err(NotificationServiceError::conflict())
            }
        };
        Ok(NotificationSettingsGetOutputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            settings: settings_dto(&record)?,
        })
    }

    fn batch_dto(
        &self,
        record: &NotificationBatchRecord,
        settings: &NotificationSettingsRecord,
        now: i64,
    ) -> Result<NotificationBatchDto, NotificationServiceError> {
        let items = self
            .storage
            .list_notification_batch_items(&record.id, now)
            .map_err(NotificationServiceError::internal)?;
        let counts = count_items(&items);
        let highest_priority = items
            .iter()
            .max_by_key(|item| priority_rank(&item.priority))
            .map(|item| item.priority.as_str())
            .unwrap_or(record.highest_priority.as_str());
        Ok(NotificationBatchDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: record.id.clone(),
            revision: nonnegative(record.revision)?,
            status: batch_status(&record.status)?,
            highest_priority: priority(highest_priority)?,
            item_count: items.len() as u64,
            items: items.iter().map(item_dto).collect::<Result<Vec<_>, _>>()?,
            counts,
            collect_until: record.collect_until,
            replace_until: record.replace_until,
            notifications_enabled: settings.enabled,
            sound_enabled: settings.sound_enabled,
            show_task_content: settings.show_task_content,
            sound_level_played: sound_level(&record.sound_level_played)?,
            delivered_revision: record.delivered_revision.map(nonnegative).transpose()?,
            delivered_priority: record
                .delivered_priority
                .as_deref()
                .map(priority)
                .transpose()?,
            is_update: record.delivered_revision.is_some(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

pub(crate) fn notification_event_dto(
    record: NotificationChangeEventRecord,
) -> Result<NotificationEventDto, NotificationServiceError> {
    Ok(NotificationEventDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        sequence: nonnegative(record.sequence)?,
        event_id: record.event_id,
        kind: match record.event_kind.as_str() {
            "created" => NotificationChangeKindDto::Created,
            "updated" => NotificationChangeKindDto::Updated,
            "seen" => NotificationChangeKindDto::Seen,
            "resolved" => NotificationChangeKindDto::Resolved,
            "settings_updated" => NotificationChangeKindDto::SettingsUpdated,
            _ => {
                return Err(NotificationServiceError::internal(
                    "Stored notification change kind is invalid",
                ))
            }
        },
        notification_id: record.notification_id,
        batch_id: record.batch_id,
        resource_revision: record.resource_revision.map(nonnegative).transpose()?,
        occurred_at: record.occurred_at,
    })
}

fn item_dto(
    r: &NotificationEventRecord,
) -> Result<NotificationListItemDto, NotificationServiceError> {
    Ok(NotificationListItemDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        event_id: r.id.clone(),
        batch_id: r.batch_id.clone(),
        kind: kind(&r.notification_kind)?,
        source_kind: match r.source_kind.as_str() {
            "human_root" => NotificationSourceKindDto::HumanRoot,
            "automation" => NotificationSourceKindDto::Automation,
            _ => {
                return Err(NotificationServiceError::internal(
                    "Stored notification source kind is invalid",
                ))
            }
        },
        source_id: r.source_id.clone(),
        run_id: r.run_id.clone(),
        automation_id: r.automation_id.clone(),
        conversation_id: r.conversation_id.clone(),
        user_message_id: r.user_message_id.clone(),
        assistant_message_id: r.assistant_message_id.clone(),
        approval_action_id: r.approval_action_id.clone(),
        subject_kind: match r.subject_kind.as_str() {
            "prompt_excerpt" => NotificationSubjectKindDto::PromptExcerpt,
            "automation_title" => NotificationSubjectKindDto::AutomationTitle,
            "attachment_task" => NotificationSubjectKindDto::AttachmentTask,
            _ => {
                return Err(NotificationServiceError::internal(
                    "Stored notification subject kind is invalid",
                ))
            }
        },
        subject_text: r.subject_text.clone(),
        priority: priority(&r.priority)?,
        resource_revision: r.resource_revision.map(nonnegative).transpose()?,
        seen_at: r.seen_at,
        resolved_at: r.resolved_at,
        occurred_at: r.occurred_at,
    })
}
fn kind(v: &str) -> Result<NotificationKindDto, NotificationServiceError> {
    Ok(match v {
        "task_completed" => NotificationKindDto::TaskCompleted,
        "task_failed" => NotificationKindDto::TaskFailed,
        "task_cancelled" => NotificationKindDto::TaskCancelled,
        "approval_required" => NotificationKindDto::ApprovalRequired,
        "automation_completed" => NotificationKindDto::AutomationCompleted,
        "automation_failed" => NotificationKindDto::AutomationFailed,
        "automation_cancelled" => NotificationKindDto::AutomationCancelled,
        "automation_important_update" => NotificationKindDto::AutomationImportantUpdate,
        "automation_configuration_blocked" => NotificationKindDto::AutomationConfigurationBlocked,
        _ => {
            return Err(NotificationServiceError::internal(
                "Stored notification kind is invalid",
            ))
        }
    })
}
fn priority(v: &str) -> Result<NotificationPriorityDto, NotificationServiceError> {
    Ok(match v {
        "completed" => NotificationPriorityDto::Completed,
        "cancelled" => NotificationPriorityDto::Cancelled,
        "important_update" => NotificationPriorityDto::ImportantUpdate,
        "failed" => NotificationPriorityDto::Failed,
        "configuration_blocked" => NotificationPriorityDto::ConfigurationBlocked,
        "approval_required" => NotificationPriorityDto::ApprovalRequired,
        _ => {
            return Err(NotificationServiceError::internal(
                "Stored notification priority is invalid",
            ))
        }
    })
}
fn priority_string(v: NotificationPriorityDto) -> &'static str {
    match v {
        NotificationPriorityDto::Completed => "completed",
        NotificationPriorityDto::Cancelled => "cancelled",
        NotificationPriorityDto::ImportantUpdate => "important_update",
        NotificationPriorityDto::Failed => "failed",
        NotificationPriorityDto::ConfigurationBlocked => "configuration_blocked",
        NotificationPriorityDto::ApprovalRequired => "approval_required",
    }
}
fn priority_rank(v: &str) -> u8 {
    match v {
        "approval_required" => 6,
        "configuration_blocked" => 5,
        "failed" => 4,
        "important_update" => 3,
        "cancelled" => 2,
        _ => 1,
    }
}
fn batch_status(v: &str) -> Result<NotificationBatchStatusDto, NotificationServiceError> {
    Ok(match v {
        "collecting" => NotificationBatchStatusDto::Collecting,
        "pending" => NotificationBatchStatusDto::Pending,
        "claimed" => NotificationBatchStatusDto::Claimed,
        "displayed" => NotificationBatchStatusDto::Displayed,
        "sealed" => NotificationBatchStatusDto::Sealed,
        "suppressed" => NotificationBatchStatusDto::Suppressed,
        _ => {
            return Err(NotificationServiceError::internal(
                "Stored notification batch status is invalid",
            ))
        }
    })
}
fn sound_level(v: &str) -> Result<NotificationSoundLevelDto, NotificationServiceError> {
    Ok(match v {
        "none" => NotificationSoundLevelDto::None,
        "initial" => NotificationSoundLevelDto::Initial,
        "upgrade" => NotificationSoundLevelDto::Upgrade,
        _ => {
            return Err(NotificationServiceError::internal(
                "Stored notification sound level is invalid",
            ))
        }
    })
}
fn sound_string(v: NotificationSoundLevelDto) -> &'static str {
    match v {
        NotificationSoundLevelDto::None => "none",
        NotificationSoundLevelDto::Initial => "initial",
        NotificationSoundLevelDto::Upgrade => "upgrade",
    }
}
fn disposition_string(v: NotificationDeliveryDispositionDto) -> &'static str {
    match v {
        NotificationDeliveryDispositionDto::Delivered => "delivered",
        NotificationDeliveryDispositionDto::SuppressedForeground => "suppressed_foreground",
        NotificationDeliveryDispositionDto::SuppressedStale => "suppressed_stale",
        NotificationDeliveryDispositionDto::SuppressedDeleted => "suppressed_deleted",
        NotificationDeliveryDispositionDto::SuppressedResolved => "suppressed_resolved",
        NotificationDeliveryDispositionDto::SuppressedDisabled => "suppressed_disabled",
    }
}
fn count_items(items: &[NotificationEventRecord]) -> NotificationCountsDto {
    let mut c = NotificationCounts::default();
    for i in items {
        match i.notification_kind.as_str() {
            "task_completed" | "automation_completed" => c.completed += 1,
            "task_failed" | "automation_failed" => c.failed += 1,
            "task_cancelled" | "automation_cancelled" => c.cancelled += 1,
            "approval_required" => c.approval_required += 1,
            "automation_important_update" => c.important_update += 1,
            "automation_configuration_blocked" => c.configuration_blocked += 1,
            _ => {}
        }
    }
    counts_dto(&c)
}
fn counts_dto(c: &NotificationCounts) -> NotificationCountsDto {
    NotificationCountsDto {
        completed: c.completed,
        failed: c.failed,
        cancelled: c.cancelled,
        approval_required: c.approval_required,
        important_update: c.important_update,
        configuration_blocked: c.configuration_blocked,
    }
}
fn summary_dto(
    v: &NotificationSummaryRecord,
) -> Result<NotificationSummaryOutputDto, NotificationServiceError> {
    Ok(NotificationSummaryOutputDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        unread_count: v.unread_count,
        unresolved_count: v.unresolved_count,
        counts: counts_dto(&v.counts),
        latest_occurred_at: v.latest_occurred_at,
        last_sequence: nonnegative(v.last_sequence)?,
    })
}
fn settings_dto(
    v: &NotificationSettingsRecord,
) -> Result<NotificationSettingsDto, NotificationServiceError> {
    Ok(NotificationSettingsDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        enabled: v.enabled,
        sound_enabled: v.sound_enabled,
        show_task_content: v.show_task_content,
        human_completed_enabled: v.human_completed_enabled,
        human_failed_enabled: v.human_failed_enabled,
        human_approval_enabled: v.human_approval_enabled,
        human_cancelled_enabled: v.human_cancelled_enabled,
        revision: nonnegative(v.revision)?,
        updated_at: v.updated_at,
    })
}
fn validate_schema(v: u32) -> Result<(), NotificationServiceError> {
    if v == NOTIFICATION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(NotificationServiceError::validation(
            "Unsupported notification schema version.",
            "schemaVersion",
        ))
    }
}
fn validate_id(v: &str, field: &'static str) -> Result<(), NotificationServiceError> {
    if !v.is_empty() && v.len() <= MAX_ID_BYTES {
        Ok(())
    } else {
        Err(NotificationServiceError::validation(
            "Identifier is invalid.",
            field,
        ))
    }
}
fn nonnegative(v: i64) -> Result<u64, NotificationServiceError> {
    u64::try_from(v)
        .map_err(|_| NotificationServiceError::internal("Stored notification integer is invalid"))
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    occurred_at: i64,
    id: String,
}
fn encode_cursor(v: &NotificationListCursor) -> Result<String, NotificationServiceError> {
    serde_json::to_vec(&Cursor {
        occurred_at: v.occurred_at,
        id: v.id.clone(),
    })
    .map(|b| URL_SAFE_NO_PAD.encode(b))
    .map_err(|_| NotificationServiceError::internal("Notification cursor could not be encoded"))
}
fn decode_cursor(v: &str) -> Result<NotificationListCursor, NotificationServiceError> {
    let b = URL_SAFE_NO_PAD
        .decode(v)
        .map_err(|_| NotificationServiceError::validation("cursor is invalid.", "cursor"))?;
    let c: Cursor = serde_json::from_slice(&b)
        .map_err(|_| NotificationServiceError::validation("cursor is invalid.", "cursor"))?;
    validate_id(&c.id, "cursor")?;
    if c.occurred_at < 0 {
        return Err(NotificationServiceError::validation(
            "cursor is invalid.",
            "cursor",
        ));
    }
    Ok(NotificationListCursor {
        occurred_at: c.occurred_at,
        id: c.id,
    })
}
