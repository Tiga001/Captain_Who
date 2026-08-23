use super::*;
use crate::storage::automation_repository::{
    AutomationAttentionListCursor, AutomationAttentionListPage, AutomationAttentionRecord,
    AutomationAttentionSummary, AutomationCompareAndSetOutcome, AutomationConfigRecord,
    AutomationCreateOutcome, AutomationEventRecord, AutomationListInput, AutomationListPage,
    AutomationNotificationRecord, AutomationRecord, AutomationRunEnqueueOutcome,
    AutomationRunListCursor, AutomationRunListPage, AutomationRunMutationOutcome,
    AutomationRunRecord, AutomationRunRecoveryRecord, AutomationRunSettlementInput,
    NewAutomationNotificationRecord, NewAutomationRecord, NewManualAutomationRunRecord,
    NewScheduledAutomationRunRecord, ScheduledAutomationRunEnqueueOutcome, StoredAutomationStatus,
};

impl StorageService {
    pub fn create_automation(
        &self,
        input: &NewAutomationRecord,
    ) -> Result<AutomationCreateOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::create_automation(&mut connection, input).map_err(storage_error)
    }

    pub fn get_automation(&self, automation_id: &str) -> Result<Option<AutomationRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation(&connection, automation_id).map_err(storage_error)
    }

    pub fn get_automation_by_create_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<AutomationRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation_by_create_request_id(&connection, request_id)
            .map_err(storage_error)
    }

    pub fn list_automations(
        &self,
        input: &AutomationListInput,
    ) -> Result<AutomationListPage, String> {
        let connection = self.state.connection()?;
        automation_repository::list_automations(&connection, input).map_err(storage_error)
    }

    pub fn replace_automation_config(
        &self,
        automation_id: &str,
        expected_revision: i64,
        config: &AutomationConfigRecord,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::replace_automation_config(
            &mut connection,
            automation_id,
            expected_revision,
            config,
        )
        .map_err(storage_error)
    }

    pub fn block_automation(
        &self,
        automation_id: &str,
        expected_revision: i64,
        blocked_code: &str,
        blocked_message: &str,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::block_automation(
            &mut connection,
            automation_id,
            expected_revision,
            blocked_code,
            blocked_message,
        )
        .map_err(storage_error)
    }

    pub fn set_automation_status(
        &self,
        automation_id: &str,
        expected_revision: i64,
        status: StoredAutomationStatus,
        resumed_next_run_at: Option<i64>,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::set_automation_status(
            &mut connection,
            automation_id,
            expected_revision,
            status,
            resumed_next_run_at,
        )
        .map_err(storage_error)
    }

    pub fn tombstone_automation(
        &self,
        automation_id: &str,
        expected_revision: i64,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::tombstone_automation(
            &mut connection,
            automation_id,
            expected_revision,
        )
        .map_err(storage_error)
    }

    pub fn enqueue_manual_automation_run(
        &self,
        input: &NewManualAutomationRunRecord,
    ) -> Result<AutomationRunEnqueueOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::enqueue_manual_automation_run(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn list_due_automations(
        &self,
        due_at_or_before: i64,
        limit: usize,
    ) -> Result<Vec<AutomationRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_due_automations(&connection, due_at_or_before, limit)
            .map_err(storage_error)
    }

    pub fn enqueue_scheduled_automation_run(
        &self,
        input: &NewScheduledAutomationRunRecord,
    ) -> Result<ScheduledAutomationRunEnqueueOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::enqueue_scheduled_automation_run(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn claim_ready_automation_runs(
        &self,
        now: i64,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::claim_ready_automation_runs(
            &mut connection,
            now,
            lease_duration_ms,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn recover_automation_admission_leases_on_startup(
        &self,
        recovered_at: i64,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::recover_automation_admission_leases_on_startup(
            &mut connection,
            recovered_at,
        )
        .map_err(storage_error)
    }

    pub fn defer_automation_run(
        &self,
        automation_run_id: &str,
        admission_token: &str,
        retry_at: i64,
        deferred_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::defer_automation_run(
            &mut connection,
            automation_run_id,
            admission_token,
            retry_at,
            deferred_at,
        )
        .map_err(storage_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn terminate_unadmitted_automation_run(
        &self,
        automation_run_id: &str,
        admission_token: &str,
        terminal_status: crate::storage::automation_repository::StoredAutomationRunStatus,
        error_code: &str,
        error_message: &str,
        settled_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::terminate_unadmitted_automation_run(
            &mut connection,
            automation_run_id,
            admission_token,
            terminal_status,
            error_code,
            error_message,
            settled_at,
        )
        .map_err(storage_error)
    }

    pub fn get_automation_run(
        &self,
        automation_run_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation_run(&connection, automation_run_id)
            .map_err(storage_error)
    }

    pub fn get_automation_run_by_agent_run_id(
        &self,
        agent_run_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation_run_by_agent_run_id(&connection, agent_run_id)
            .map_err(storage_error)
    }

    pub fn record_automation_report(
        &self,
        automation_run_id: &str,
        report_kind: &str,
        summary: &str,
        reported_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::record_automation_report(
            &mut connection,
            automation_run_id,
            report_kind,
            summary,
            reported_at,
        )
        .map_err(storage_error)
    }

    pub fn set_automation_run_waiting_for_approval(
        &self,
        automation_run_id: &str,
        agent_run_id: &str,
        waiting: bool,
        changed_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::set_automation_run_waiting_for_approval(
            &mut connection,
            automation_run_id,
            agent_run_id,
            waiting,
            changed_at,
        )
        .map_err(storage_error)
    }

    pub fn settle_automation_run_from_trace(
        &self,
        input: &AutomationRunSettlementInput,
    ) -> Result<AutomationRunMutationOutcome, String> {
        let mut connection = self.state.connection()?;
        automation_repository::settle_automation_run_from_trace(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn list_recoverable_automation_runs(
        &self,
    ) -> Result<Vec<AutomationRunRecoveryRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_recoverable_automation_runs(&connection).map_err(storage_error)
    }

    pub fn list_cancellation_requested_automation_runs(
        &self,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_cancellation_requested_automation_runs(&connection)
            .map_err(storage_error)
    }

    pub fn enqueue_automation_notification(
        &self,
        input: &NewAutomationNotificationRecord,
    ) -> Result<AutomationNotificationRecord, String> {
        let mut connection = self.state.connection()?;
        automation_repository::enqueue_automation_notification(&mut connection, input)
            .map_err(storage_error)
    }

    pub fn claim_pending_automation_notifications(
        &self,
        claim_token: &str,
        now: i64,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<AutomationNotificationRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::claim_pending_automation_notifications(
            &mut connection,
            claim_token,
            now,
            lease_duration_ms,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn acknowledge_automation_notification_delivered(
        &self,
        notification_id: &str,
        claim_token: &str,
        delivered_at: i64,
    ) -> Result<Option<AutomationNotificationRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::acknowledge_automation_notification_delivered(
            &mut connection,
            notification_id,
            claim_token,
            delivered_at,
        )
        .map_err(storage_error)
    }

    pub fn validate_claimed_automation_notification(
        &self,
        notification_id: &str,
        claim_token: &str,
        now: i64,
    ) -> Result<Option<AutomationNotificationRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::validate_claimed_automation_notification(
            &mut connection,
            notification_id,
            claim_token,
            now,
        )
        .map_err(storage_error)
    }

    pub fn release_automation_notification(
        &self,
        notification_id: &str,
        claim_token: &str,
        retry_at: i64,
        error_code: &str,
    ) -> Result<Option<AutomationNotificationRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::release_automation_notification(
            &mut connection,
            notification_id,
            claim_token,
            retry_at,
            error_code,
        )
        .map_err(storage_error)
    }

    pub fn suppress_automation_notification(
        &self,
        notification_id: &str,
        suppressed_at: i64,
    ) -> Result<Option<AutomationNotificationRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::suppress_automation_notification(
            &mut connection,
            notification_id,
            suppressed_at,
        )
        .map_err(storage_error)
    }

    pub fn list_automation_runs(
        &self,
        automation_id: &str,
        cursor: Option<&AutomationRunListCursor>,
        limit: usize,
    ) -> Result<AutomationRunListPage, String> {
        let connection = self.state.connection()?;
        automation_repository::list_automation_runs(&connection, automation_id, cursor, limit)
            .map_err(storage_error)
    }

    pub fn get_latest_automation_run(
        &self,
        automation_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_latest_automation_run(&connection, automation_id)
            .map_err(storage_error)
    }

    pub fn get_automation_run_by_manual_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation_run_by_manual_request_id(&connection, request_id)
            .map_err(storage_error)
    }

    pub fn automation_attention_summary(&self) -> Result<AutomationAttentionSummary, String> {
        let connection = self.state.connection()?;
        automation_repository::automation_attention_summary(&connection).map_err(storage_error)
    }

    pub fn list_automation_attentions(
        &self,
        cursor: Option<&AutomationAttentionListCursor>,
        limit: usize,
    ) -> Result<AutomationAttentionListPage, String> {
        let connection = self.state.connection()?;
        automation_repository::list_automation_attentions(&connection, cursor, limit)
            .map_err(storage_error)
    }

    pub fn acknowledge_automation_attention(
        &self,
        automation_id: &str,
        automation_run_id: Option<&str>,
        acknowledged_at: i64,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        automation_repository::acknowledge_automation_attention(
            &mut connection,
            automation_id,
            automation_run_id,
            acknowledged_at,
        )
        .map_err(storage_error)
    }

    pub fn acknowledge_automation_attention_id(
        &self,
        attention_id: &str,
        acknowledged_at: i64,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        automation_repository::acknowledge_automation_attention_id(
            &mut connection,
            attention_id,
            acknowledged_at,
        )
        .map_err(storage_error)
    }

    pub fn get_automation_attention_by_id(
        &self,
        attention_id: &str,
    ) -> Result<Option<AutomationAttentionRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_automation_attention_by_id(&connection, attention_id)
            .map_err(storage_error)
    }

    pub fn acknowledge_automation_attention_record(
        &self,
        attention_id: &str,
        acknowledged_at: i64,
    ) -> Result<Option<AutomationAttentionRecord>, String> {
        let mut connection = self.state.connection()?;
        automation_repository::acknowledge_automation_attention_record(
            &mut connection,
            attention_id,
            acknowledged_at,
        )
        .map_err(storage_error)
    }

    pub fn list_automation_events_after(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<AutomationEventRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_automation_events_after(&connection, after_sequence, limit)
            .map_err(storage_error)
    }

    pub fn get_latest_automation_event(&self) -> Result<Option<AutomationEventRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_latest_automation_event(&connection).map_err(storage_error)
    }

    pub fn get_latest_automation_event_for(
        &self,
        automation_id: &str,
        automation_run_id: Option<&str>,
        event_kind: &str,
        resource_revision: Option<i64>,
    ) -> Result<Option<AutomationEventRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::get_latest_automation_event_for(
            &connection,
            automation_id,
            automation_run_id,
            event_kind,
            resource_revision,
        )
        .map_err(storage_error)
    }

    pub fn latest_automation_event_sequence(&self) -> Result<i64, String> {
        let connection = self.state.connection()?;
        automation_repository::latest_automation_event_sequence(&connection).map_err(storage_error)
    }

    pub fn list_nonterminal_automation_runs(&self) -> Result<Vec<AutomationRunRecord>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_nonterminal_automation_runs(&connection).map_err(storage_error)
    }

    pub fn list_nonterminal_automation_agent_run_ids_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<String>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_nonterminal_automation_agent_run_ids_for_project(
            &connection,
            project_id,
        )
        .map_err(storage_error)
    }

    pub fn list_nonterminal_automation_agent_run_ids_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<String>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_nonterminal_automation_agent_run_ids_for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)
    }

    pub fn list_nonterminal_automation_agent_run_ids_for_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<Vec<String>, String> {
        let connection = self.state.connection()?;
        automation_repository::list_nonterminal_automation_agent_run_ids_for_messages(
            &connection,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)
    }
}
