use super::*;
use crate::storage::automation_repository::{
    AutomationAttentionListCursor, AutomationAttentionListPage, AutomationAttentionRecord,
    AutomationAttentionSummary, AutomationCompareAndSetOutcome, AutomationConfigRecord,
    AutomationCreateOutcome, AutomationEventRecord, AutomationListInput, AutomationListPage,
    AutomationRecord, AutomationRunEnqueueOutcome, AutomationRunListCursor, AutomationRunListPage,
    AutomationRunMutationOutcome, AutomationRunRecord, AutomationRunRecoveryRecord,
    AutomationRunSettlementInput, NewAutomationRecord, NewManualAutomationRunRecord,
    NewScheduledAutomationRunRecord, ScheduledAutomationRunEnqueueOutcome, StoredAutomationStatus,
};
use crate::storage::StorageWriteClass;

impl StorageService {
    pub fn create_automation(
        &self,
        input: &NewAutomationRecord,
    ) -> Result<AutomationCreateOutcome, String> {
        self.state.write_sync(
            automation_config_bytes(&input.config).saturating_add(automation_payload_bytes(&[
                &input.id,
                &input.create_request_id,
            ])),
            StorageWriteClass::Normal,
            |connection| {
                automation_repository::create_automation(connection, input).map_err(storage_error)
            },
        )?
    }

    pub fn get_automation(&self, automation_id: &str) -> Result<Option<AutomationRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_automation(&connection, automation_id).map_err(storage_error)
    }

    pub fn get_automation_by_create_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<AutomationRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_automation_by_create_request_id(&connection, request_id)
            .map_err(storage_error)
    }

    pub fn list_automations(
        &self,
        input: &AutomationListInput,
    ) -> Result<AutomationListPage, String> {
        let mut connection = self.state.read_connection()?;
        // Rows, aggregate counts, and the durable event cursor describe the same snapshot.
        let transaction = connection.transaction().map_err(storage_error)?;
        let result =
            automation_repository::list_automations(&transaction, input).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn replace_automation_config(
        &self,
        automation_id: &str,
        expected_revision: i64,
        config: &AutomationConfigRecord,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        self.state.write_sync(
            automation_config_bytes(config).saturating_add(automation_id.len()),
            StorageWriteClass::Normal,
            |connection| {
                automation_repository::replace_automation_config(
                    connection,
                    automation_id,
                    expected_revision,
                    config,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn block_automation(
        &self,
        automation_id: &str,
        expected_revision: i64,
        blocked_code: &str,
        blocked_message: &str,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_id, blocked_code, blocked_message]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::block_automation(
                    connection,
                    automation_id,
                    expected_revision,
                    blocked_code,
                    blocked_message,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn set_automation_status(
        &self,
        automation_id: &str,
        expected_revision: i64,
        status: StoredAutomationStatus,
        resumed_next_run_at: Option<i64>,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_id]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::set_automation_status(
                    connection,
                    automation_id,
                    expected_revision,
                    status,
                    resumed_next_run_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn tombstone_automation(
        &self,
        automation_id: &str,
        expected_revision: i64,
    ) -> Result<AutomationCompareAndSetOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_id]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::tombstone_automation(
                    connection,
                    automation_id,
                    expected_revision,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn enqueue_manual_automation_run(
        &self,
        input: &NewManualAutomationRunRecord,
    ) -> Result<AutomationRunEnqueueOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[
                &input.id,
                &input.automation_id,
                &input.manual_request_id,
                &input.config_snapshot_json,
            ]),
            StorageWriteClass::Normal,
            |connection| {
                automation_repository::enqueue_manual_automation_run(connection, input)
                    .map_err(storage_error)
            },
        )?
    }

    pub fn list_due_automations(
        &self,
        due_at_or_before: i64,
        limit: usize,
    ) -> Result<Vec<AutomationRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_due_automations(&connection, due_at_or_before, limit)
            .map_err(storage_error)
    }

    pub fn enqueue_scheduled_automation_run(
        &self,
        input: &NewScheduledAutomationRunRecord,
    ) -> Result<ScheduledAutomationRunEnqueueOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[
                &input.id,
                &input.automation_id,
                &input.trigger_kind,
                &input.config_snapshot_json,
            ]),
            StorageWriteClass::Normal,
            |connection| {
                automation_repository::enqueue_scheduled_automation_run(connection, input)
                    .map_err(storage_error)
            },
        )?
    }

    pub fn claim_ready_automation_runs(
        &self,
        now: i64,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        self.state
            .write_sync(1024, StorageWriteClass::Control, |connection| {
                automation_repository::claim_ready_automation_runs(
                    connection,
                    now,
                    lease_duration_ms,
                    limit,
                )
                .map_err(storage_error)
            })?
    }

    /// Begins the production lease only after bounded writer admission and SQLite lock acquisition.
    pub fn claim_ready_automation_runs_now(
        &self,
        lease_duration_ms: i64,
        limit: usize,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        self.state
            .write_sync(1024, StorageWriteClass::Control, |connection| {
                automation_repository::claim_ready_automation_runs_now(
                    connection,
                    lease_duration_ms,
                    limit,
                )
                .map_err(storage_error)
            })?
    }

    pub fn recover_automation_admission_leases_on_startup(
        &self,
        recovered_at: i64,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        self.state
            .write_sync(1024, StorageWriteClass::Control, |connection| {
                automation_repository::recover_automation_admission_leases_on_startup(
                    connection,
                    recovered_at,
                )
                .map_err(storage_error)
            })?
    }

    pub fn defer_automation_run(
        &self,
        automation_run_id: &str,
        admission_token: &str,
        retry_at: i64,
        deferred_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_run_id, admission_token]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::defer_automation_run(
                    connection,
                    automation_run_id,
                    admission_token,
                    retry_at,
                    deferred_at,
                )
                .map_err(storage_error)
            },
        )?
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
        self.state.write_sync(
            automation_payload_bytes(&[
                automation_run_id,
                admission_token,
                error_code,
                error_message,
            ]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::terminate_unadmitted_automation_run(
                    connection,
                    automation_run_id,
                    admission_token,
                    terminal_status,
                    error_code,
                    error_message,
                    settled_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn get_automation_run(
        &self,
        automation_run_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_automation_run(&connection, automation_run_id)
            .map_err(storage_error)
    }

    pub fn fail_unadmitted_automation_runs_for_execution_access(
        &self,
        run_id: Option<&str>,
        code: &str,
        message: &str,
        settled_at: i64,
        limit: usize,
    ) -> Result<usize, String> {
        self.state.write_sync(
            automation_payload_bytes(&[run_id.unwrap_or_default(), code, message]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::fail_unadmitted_automation_runs_for_execution_access(
                    connection, run_id, code, message, settled_at, limit,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn get_automation_run_by_agent_run_id(
        &self,
        agent_run_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
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
        self.state.write_sync(
            automation_payload_bytes(&[automation_run_id, report_kind, summary]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::record_automation_report(
                    connection,
                    automation_run_id,
                    report_kind,
                    summary,
                    reported_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn set_automation_run_waiting_for_approval(
        &self,
        automation_run_id: &str,
        agent_run_id: &str,
        waiting: bool,
        changed_at: i64,
    ) -> Result<AutomationRunMutationOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_run_id, agent_run_id]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::set_automation_run_waiting_for_approval(
                    connection,
                    automation_run_id,
                    agent_run_id,
                    waiting,
                    changed_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn settle_automation_run_from_trace(
        &self,
        input: &AutomationRunSettlementInput,
    ) -> Result<AutomationRunMutationOutcome, String> {
        self.state.write_sync(
            automation_payload_bytes(&[
                &input.automation_run_id,
                &input.agent_run_id,
                input.report_kind.as_deref().unwrap_or_default(),
                input.result_preview.as_deref().unwrap_or_default(),
                input.error_code.as_deref().unwrap_or_default(),
                input.error_message.as_deref().unwrap_or_default(),
            ]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::settle_automation_run_from_trace(connection, input)
                    .map_err(storage_error)
            },
        )?
    }

    pub fn list_recoverable_automation_runs(
        &self,
    ) -> Result<Vec<AutomationRunRecoveryRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_recoverable_automation_runs(&connection).map_err(storage_error)
    }

    pub fn list_cancellation_requested_automation_runs(
        &self,
    ) -> Result<Vec<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_cancellation_requested_automation_runs(&connection)
            .map_err(storage_error)
    }

    pub fn list_automation_runs(
        &self,
        automation_id: &str,
        cursor: Option<&AutomationRunListCursor>,
        limit: usize,
    ) -> Result<AutomationRunListPage, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_automation_runs(&connection, automation_id, cursor, limit)
            .map_err(storage_error)
    }

    pub fn get_latest_automation_run(
        &self,
        automation_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_latest_automation_run(&connection, automation_id)
            .map_err(storage_error)
    }

    pub fn get_automation_run_by_manual_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_automation_run_by_manual_request_id(&connection, request_id)
            .map_err(storage_error)
    }

    pub fn automation_attention_summary(&self) -> Result<AutomationAttentionSummary, String> {
        let mut connection = self.state.read_connection()?;
        // Rows, aggregate counts, and the durable event cursor describe the same snapshot.
        let transaction = connection.transaction().map_err(storage_error)?;
        let result = automation_repository::automation_attention_summary(&transaction)
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(result)
    }

    pub fn list_automation_attentions(
        &self,
        cursor: Option<&AutomationAttentionListCursor>,
        limit: usize,
    ) -> Result<AutomationAttentionListPage, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_automation_attentions(&connection, cursor, limit)
            .map_err(storage_error)
    }

    pub fn acknowledge_automation_attention(
        &self,
        automation_id: &str,
        automation_run_id: Option<&str>,
        acknowledged_at: i64,
    ) -> Result<bool, String> {
        self.state.write_sync(
            automation_payload_bytes(&[automation_id, automation_run_id.unwrap_or_default()]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::acknowledge_automation_attention(
                    connection,
                    automation_id,
                    automation_run_id,
                    acknowledged_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn acknowledge_automation_attention_id(
        &self,
        attention_id: &str,
        acknowledged_at: i64,
    ) -> Result<bool, String> {
        self.state.write_sync(
            automation_payload_bytes(&[attention_id]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::acknowledge_automation_attention_id(
                    connection,
                    attention_id,
                    acknowledged_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn get_automation_attention_by_id(
        &self,
        attention_id: &str,
    ) -> Result<Option<AutomationAttentionRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_automation_attention_by_id(&connection, attention_id)
            .map_err(storage_error)
    }

    pub fn acknowledge_automation_attention_record(
        &self,
        attention_id: &str,
        acknowledged_at: i64,
    ) -> Result<Option<AutomationAttentionRecord>, String> {
        self.state.write_sync(
            automation_payload_bytes(&[attention_id]),
            StorageWriteClass::Control,
            |connection| {
                automation_repository::acknowledge_automation_attention_record(
                    connection,
                    attention_id,
                    acknowledged_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn list_automation_events_after(
        &self,
        after_sequence: i64,
        limit: usize,
    ) -> Result<Vec<AutomationEventRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_automation_events_after(&connection, after_sequence, limit)
            .map_err(storage_error)
    }

    pub fn get_latest_automation_event(&self) -> Result<Option<AutomationEventRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::get_latest_automation_event(&connection).map_err(storage_error)
    }

    pub fn get_latest_automation_event_for(
        &self,
        automation_id: &str,
        automation_run_id: Option<&str>,
        event_kind: &str,
        resource_revision: Option<i64>,
    ) -> Result<Option<AutomationEventRecord>, String> {
        let connection = self.state.read_connection()?;
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
        let connection = self.state.read_connection()?;
        automation_repository::latest_automation_event_sequence(&connection).map_err(storage_error)
    }

    pub fn list_nonterminal_automation_runs(&self) -> Result<Vec<AutomationRunRecord>, String> {
        let connection = self.state.read_connection()?;
        automation_repository::list_nonterminal_automation_runs(&connection).map_err(storage_error)
    }

    pub fn list_nonterminal_automation_agent_run_ids_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<String>, String> {
        let connection = self.state.read_connection()?;
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
        let connection = self.state.read_connection()?;
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
        let connection = self.state.read_connection()?;
        automation_repository::list_nonterminal_automation_agent_run_ids_for_messages(
            &connection,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)
    }
}

// Account for every queued string without allocating a duplicate serialized representation.
// Fixed metadata is bounded separately; these are admission estimates, not total RSS bounds.
fn automation_payload_bytes(values: &[&str]) -> usize {
    values
        .iter()
        .fold(1024usize, |size, value| size.saturating_add(value.len()))
}

fn automation_config_bytes(config: &AutomationConfigRecord) -> usize {
    let AutomationConfigRecord {
        title,
        prompt,
        health_state,
        blocked_code,
        blocked_message,
        destination_kind,
        target_conversation_id,
        project_binding_kind,
        project_id,
        model_id,
        permission_mode,
        permission_mode_version: _,
        permissions_json,
        reasoning_json,
        schedule_kind,
        schedule_json,
        rrule,
        timezone,
        anchor_at: _,
        next_run_at: _,
        notification_policy,
        target_project_snapshot,
        target_conversation_snapshot,
        target_model_snapshot,
        target_project_id_snapshot,
        target_conversation_id_snapshot,
        target_model_id_snapshot,
    } = config;
    automation_payload_bytes(&[
        title,
        prompt,
        health_state,
        blocked_code.as_deref().unwrap_or_default(),
        blocked_message.as_deref().unwrap_or_default(),
        destination_kind,
        target_conversation_id.as_deref().unwrap_or_default(),
        project_binding_kind,
        project_id.as_deref().unwrap_or_default(),
        model_id.as_deref().unwrap_or_default(),
        permission_mode,
        permissions_json,
        reasoning_json.as_deref().unwrap_or_default(),
        schedule_kind,
        schedule_json,
        rrule,
        timezone,
        notification_policy,
        target_project_snapshot.as_deref().unwrap_or_default(),
        target_conversation_snapshot.as_deref().unwrap_or_default(),
        target_model_snapshot.as_deref().unwrap_or_default(),
        target_project_id_snapshot.as_deref().unwrap_or_default(),
        target_conversation_id_snapshot
            .as_deref()
            .unwrap_or_default(),
        target_model_id_snapshot.as_deref().unwrap_or_default(),
    ])
}

#[cfg(test)]
mod tests;
