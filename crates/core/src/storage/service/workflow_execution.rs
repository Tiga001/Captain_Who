use super::StorageService;
use crate::storage::workflow_execution_repository as repository;
use crate::workflow_execution::*;

impl StorageService {
    pub fn workflow_execution_discard_failed(&self, input_id: &str) -> Result<(), String> {
        repository::discard_failed(&mut *self.state.connection()?, input_id)
    }

    pub fn workflow_execution_mark_run_unread(&self, run_id: &str) -> Result<(), String> {
        repository::mark_run_unread(&mut *self.state.connection()?, run_id)
    }

    pub fn workflow_execution_snapshot_for_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::snapshot_for_run(&*self.state.connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_delivery_origins(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<(String, Input)>, String> {
        repository::delivery_origins_for_conversation(&*self.state.connection()?, conversation_id)
    }

    pub fn workflow_execution_recover_claims(&self) -> Result<(), String> {
        repository::recover_claims(&mut *self.state.connection()?)
    }
    pub fn workflow_execution_bind_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::bind_run(&mut *self.state.connection()?, conversation_id, run_id)
    }
    pub fn workflow_execution_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationSnapshot>, String> {
        repository::snapshot_for_conversation(&*self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_send(&self, request: &SendRequest) -> Result<SendReceipt, String> {
        repository::send(&mut *self.state.connection()?, request)
    }
    pub fn workflow_execution_pending_inputs(&self) -> Result<Vec<Input>, String> {
        repository::pending_inputs(&*self.state.connection()?)
    }
    pub fn workflow_execution_bound_inputs(&self, run_id: &str) -> Result<Vec<Input>, String> {
        repository::bound_inputs(&*self.state.connection()?, run_id)
    }
    pub fn workflow_execution_load_input(&self, input_id: &str) -> Result<Option<Input>, String> {
        repository::load_input(&*self.state.connection()?, input_id)
    }
    pub fn workflow_execution_inputs_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<Input>, String> {
        repository::inputs_for_conversation(&*self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_bind_input(
        &self,
        input_id: &str,
        run_id: &str,
        delivery_id: &str,
    ) -> Result<bool, String> {
        repository::bind_input(
            &mut *self.state.connection()?,
            input_id,
            run_id,
            delivery_id,
        )
    }
    pub fn workflow_execution_mark_applied(&self, input_id: &str) -> Result<(), String> {
        repository::mark_applied(&mut *self.state.connection()?, input_id)
    }
    pub fn workflow_execution_fail_input(
        &self,
        input_id: &str,
        reason: &str,
    ) -> Result<(), String> {
        repository::fail_input(&mut *self.state.connection()?, input_id, reason)
    }
    pub fn workflow_execution_pause_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        repository::pause_conversation(&mut *self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_resume_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<(), String> {
        repository::resume_conversation(&mut *self.state.connection()?, conversation_id)
    }
    pub fn workflow_execution_runtime(&self, instance_id: &str) -> Result<RuntimeSnapshot, String> {
        repository::runtime_snapshot(&*self.state.connection()?, instance_id, None)
    }
    pub fn workflow_execution_runtime_since(
        &self,
        instance_id: &str,
        after_sequence: Option<u64>,
    ) -> Result<RuntimeSnapshot, String> {
        repository::runtime_snapshot(&*self.state.connection()?, instance_id, after_sequence)
    }
    pub fn workflow_execution_complete_user(&self, input_id: &str) -> Result<(), String> {
        repository::complete_user_input(&mut *self.state.connection()?, input_id)
    }
}
