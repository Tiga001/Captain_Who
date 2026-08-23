use super::service::{validate_id, validate_schema, AutomationServiceError};
use mycopilot_core::storage::automation_repository::AutomationNotificationRecord;
use mycopilot_core::storage::now_ms;
use mycopilot_core::storage::service::StorageService;
use mycopilot_protocol_rs::{
    AutomationNotificationAcknowledgeInputDto, AutomationNotificationAcknowledgeOutputDto,
    AutomationNotificationDeliveredStatusDto, AutomationNotificationDeliveryDto,
    AutomationNotificationKindDto, AutomationNotificationPendingStatusDto,
    AutomationNotificationReleaseInputDto, AutomationNotificationReleaseOutputDto,
    AutomationNotificationValidateInputDto, AutomationNotificationValidateOutputDto,
    AutomationNotificationsClaimInputDto, AutomationNotificationsClaimOutputDto,
    AUTOMATION_SCHEMA_VERSION,
};

const MAX_BATCH_SIZE: u32 = 10;
const MIN_LEASE_DURATION_MS: u64 = 5_000;
const MAX_LEASE_DURATION_MS: u64 = 300_000;

/// Core-owned boundary used only by Electron Main to consume the durable native-notification
/// outbox. Renderer code cannot invoke these methods.
pub(crate) struct AutomationNotificationService<'a> {
    storage: &'a StorageService,
}

impl<'a> AutomationNotificationService<'a> {
    pub(crate) fn new(storage: &'a StorageService) -> Self {
        Self { storage }
    }

    pub(crate) fn claim(
        &self,
        input: AutomationNotificationsClaimInputDto,
    ) -> Result<AutomationNotificationsClaimOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.claim_token, "claimToken")?;
        if !(MIN_LEASE_DURATION_MS..=MAX_LEASE_DURATION_MS).contains(&input.lease_duration_ms) {
            return Err(AutomationServiceError::validation(
                "leaseDurationMs is outside the supported range.",
                Some("leaseDurationMs"),
            ));
        }
        if !(1..=MAX_BATCH_SIZE).contains(&input.limit) {
            return Err(AutomationServiceError::validation(
                "limit must be between 1 and 10.",
                Some("limit"),
            ));
        }
        let lease_duration_ms = i64::try_from(input.lease_duration_ms).map_err(|_| {
            AutomationServiceError::validation(
                "leaseDurationMs is invalid.",
                Some("leaseDurationMs"),
            )
        })?;
        let records = self
            .storage
            .claim_pending_automation_notifications(
                &input.claim_token,
                now_ms(),
                lease_duration_ms,
                input.limit as usize,
            )
            .map_err(AutomationServiceError::internal)?;
        let notifications = records
            .iter()
            .map(notification_dto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AutomationNotificationsClaimOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            claim_token: input.claim_token,
            notifications,
        })
    }

    pub(crate) fn acknowledge(
        &self,
        input: AutomationNotificationAcknowledgeInputDto,
    ) -> Result<AutomationNotificationAcknowledgeOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.notification_id, "notificationId")?;
        validate_id(&input.claim_token, "claimToken")?;
        let delivered_at = now_ms();
        let record = self
            .storage
            .acknowledge_automation_notification_delivered(
                &input.notification_id,
                &input.claim_token,
                delivered_at,
            )
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| {
                AutomationServiceError::validation(
                    "The notification delivery claim is no longer active.",
                    Some("claimToken"),
                )
            })?;
        Ok(AutomationNotificationAcknowledgeOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            notification_id: record.id,
            status: AutomationNotificationDeliveredStatusDto::Delivered,
            delivered_at: record.delivered_at.unwrap_or(delivered_at),
        })
    }

    pub(crate) fn validate(
        &self,
        input: AutomationNotificationValidateInputDto,
    ) -> Result<AutomationNotificationValidateOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.notification_id, "notificationId")?;
        validate_id(&input.claim_token, "claimToken")?;
        let notification = self
            .storage
            .validate_claimed_automation_notification(
                &input.notification_id,
                &input.claim_token,
                now_ms(),
            )
            .map_err(AutomationServiceError::internal)?
            .as_ref()
            .map(notification_dto)
            .transpose()?;
        Ok(AutomationNotificationValidateOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            notification_id: input.notification_id,
            notification,
        })
    }

    pub(crate) fn release(
        &self,
        input: AutomationNotificationReleaseInputDto,
    ) -> Result<AutomationNotificationReleaseOutputDto, AutomationServiceError> {
        validate_schema(input.schema_version)?;
        validate_id(&input.notification_id, "notificationId")?;
        validate_id(&input.claim_token, "claimToken")?;
        if input.retry_at < 0 {
            return Err(AutomationServiceError::validation(
                "retryAt must be a non-negative integer timestamp.",
                Some("retryAt"),
            ));
        }
        let record = self
            .storage
            .release_automation_notification(
                &input.notification_id,
                &input.claim_token,
                input.retry_at,
                "native_notification_failed",
            )
            .map_err(AutomationServiceError::internal)?
            .ok_or_else(|| {
                AutomationServiceError::validation(
                    "The notification delivery claim is no longer active.",
                    Some("claimToken"),
                )
            })?;
        Ok(AutomationNotificationReleaseOutputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            notification_id: record.id,
            status: AutomationNotificationPendingStatusDto::Pending,
            retry_at: record.retry_at,
        })
    }
}

fn notification_dto(
    record: &AutomationNotificationRecord,
) -> Result<AutomationNotificationDeliveryDto, AutomationServiceError> {
    let kind = match record.notification_kind.as_str() {
        "run_result" => AutomationNotificationKindDto::RunResult,
        "approval_required" => AutomationNotificationKindDto::ApprovalRequired,
        "configuration_blocked" => AutomationNotificationKindDto::ConfigurationBlocked,
        _ => {
            return Err(AutomationServiceError::internal(
                "Stored notification kind is invalid",
            ))
        }
    };
    Ok(AutomationNotificationDeliveryDto {
        schema_version: AUTOMATION_SCHEMA_VERSION,
        notification_id: record.id.clone(),
        automation_id: record.automation_id.clone(),
        run_id: record.automation_run_id.clone(),
        kind,
        title: record.title.clone(),
        body: record.body.clone(),
        conversation_id: record.conversation_id.clone(),
        user_message_id: record.user_message_id.clone(),
        assistant_message_id: record.assistant_message_id.clone(),
        created_at: record.created_at,
    })
}
