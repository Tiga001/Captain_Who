use super::*;
use crate::storage::notification_repository::{
    self, NewNotificationEventRecord, NotificationBatchRecord, NotificationChangeEventRecord,
    NotificationEventRecord, NotificationListCursor, NotificationListPage,
    NotificationSettingsRecord, NotificationSettingsUpdate, NotificationSettingsUpdateOutcome,
    NotificationSummaryRecord,
};

impl StorageService {
    pub fn enqueue_notification_event(
        &self,
        input: &NewNotificationEventRecord,
    ) -> Result<NotificationEventRecord, String> {
        let mut connection = self.state.connection()?;
        notification_repository::enqueue_notification_event(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn claim_pending_notification_batches(
        &self,
        claim_token: &str,
        now: i64,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<NotificationBatchRecord>, String> {
        let mut connection = self.state.connection()?;
        notification_repository::claim_pending_notification_batches(
            &mut connection,
            claim_token,
            now,
            lease_duration_ms,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn validate_claimed_notification_batch(
        &self,
        batch_id: &str,
        claim_token: &str,
        now: i64,
    ) -> Result<Option<NotificationBatchRecord>, String> {
        let mut connection = self.state.connection()?;
        notification_repository::validate_claimed_notification_batch(
            &mut connection,
            batch_id,
            claim_token,
            now,
        )
        .map_err(storage_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn acknowledge_notification_batch(
        &self,
        batch_id: &str,
        claim_token: &str,
        disposition: &str,
        native_priority: &str,
        sound_level_played: &str,
        native_revision: i64,
        acknowledged_at: i64,
    ) -> Result<Option<NotificationBatchRecord>, String> {
        let mut connection = self.state.connection()?;
        notification_repository::acknowledge_notification_batch(
            &mut connection,
            batch_id,
            claim_token,
            disposition,
            native_priority,
            sound_level_played,
            native_revision,
            acknowledged_at,
        )
        .map_err(storage_error)
    }

    pub fn release_notification_batch(
        &self,
        batch_id: &str,
        claim_token: &str,
        retry_at: i64,
        error_code: &str,
    ) -> Result<Option<NotificationBatchRecord>, String> {
        let mut connection = self.state.connection()?;
        notification_repository::release_notification_batch(
            &mut connection,
            batch_id,
            claim_token,
            retry_at,
            error_code,
        )
        .map_err(storage_error)
    }

    pub fn suppress_notification_batch(
        &self,
        batch_id: &str,
        reason: &str,
        suppressed_at: i64,
    ) -> Result<Option<NotificationBatchRecord>, String> {
        let mut connection = self.state.connection()?;
        notification_repository::suppress_notification_batch(
            &mut connection,
            batch_id,
            reason,
            suppressed_at,
        )
        .map_err(storage_error)
    }

    pub fn list_notification_batch_items(
        &self,
        batch_id: &str,
        now: i64,
    ) -> Result<Vec<NotificationEventRecord>, String> {
        let connection = self.state.connection()?;
        notification_repository::list_notification_batch_items(&connection, batch_id, now)
            .map_err(storage_error)
    }

    pub fn list_notifications(
        &self,
        cursor: Option<&NotificationListCursor>,
        limit: usize,
        unread_only: bool,
        batch_id: Option<&str>,
    ) -> Result<NotificationListPage, String> {
        let connection = self.state.connection()?;
        notification_repository::list_notifications(
            &connection,
            cursor,
            limit,
            unread_only,
            batch_id,
        )
        .map_err(storage_error)
    }

    pub fn notification_summary(&self) -> Result<NotificationSummaryRecord, String> {
        let connection = self.state.connection()?;
        notification_repository::notification_summary(&connection).map_err(storage_error)
    }

    pub fn mark_notification_events_seen(
        &self,
        event_ids: Option<&[String]>,
        batch_id: Option<&str>,
        all: bool,
        seen_at: i64,
    ) -> Result<usize, String> {
        let mut connection = self.state.connection()?;
        notification_repository::mark_notification_events_seen(
            &mut connection,
            event_ids,
            batch_id,
            all,
            seen_at,
        )
        .map_err(storage_error)
    }

    pub fn load_notification_settings(&self) -> Result<NotificationSettingsRecord, String> {
        let connection = self.state.connection()?;
        notification_repository::load_notification_settings(&connection).map_err(storage_error)
    }

    pub fn update_notification_settings(
        &self,
        input: &NotificationSettingsUpdate,
    ) -> Result<NotificationSettingsUpdateOutcome, String> {
        let mut connection = self.state.connection()?;
        notification_repository::update_notification_settings(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn resolve_notification_events_by_supersession_key(
        &self,
        supersession_key: &str,
        resolved_at: i64,
    ) -> Result<usize, String> {
        let mut connection = self.state.connection()?;
        notification_repository::resolve_notification_events_by_supersession_key(
            &mut connection,
            supersession_key,
            resolved_at,
        )
        .map_err(storage_error)
    }

    pub fn resolve_notification_events_by_approval_action_id(
        &self,
        approval_action_id: &str,
        resolved_at: i64,
    ) -> Result<usize, String> {
        let mut connection = self.state.connection()?;
        notification_repository::resolve_notification_events_by_approval_action_id(
            &mut connection,
            approval_action_id,
            resolved_at,
        )
        .map_err(storage_error)
    }

    pub fn list_notification_change_events_after(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<NotificationChangeEventRecord>, String> {
        let connection = self.state.connection()?;
        notification_repository::list_notification_change_events_after(
            &connection,
            after_sequence,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn latest_notification_change_sequence(&self) -> Result<i64, String> {
        let connection = self.state.connection()?;
        notification_repository::latest_notification_change_sequence(&connection)
            .map_err(storage_error)
    }
}
