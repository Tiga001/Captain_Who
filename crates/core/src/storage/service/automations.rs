use super::*;
use crate::storage::automation_repository::{
    AutomationAttentionListCursor, AutomationAttentionListPage, AutomationAttentionRecord,
    AutomationAttentionSummary, AutomationCompareAndSetOutcome, AutomationConfigRecord,
    AutomationCreateOutcome, AutomationEventRecord, AutomationListInput, AutomationListPage,
    AutomationRecord, AutomationRunEnqueueOutcome, AutomationRunListCursor, AutomationRunListPage,
    AutomationRunRecord, NewAutomationRecord, NewManualAutomationRunRecord, StoredAutomationStatus,
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
}
