//! Trusted Host ports for cross-conversation workflow collaboration.
//!
//! None of these execution identities are deserializable model arguments. A workflow snapshot
//! describes capabilities; admission still rechecks current bindings in the Host transaction.
use crate::workflow_awareness::{MailboxQuery, StateQuery};
use crate::workflow_execution::{
    ConversationSnapshot as WorkflowConversationSnapshot, SendOutput as WorkflowSendOutput,
    SendReceipt as WorkflowSendReceipt,
};
use crate::{AgentResult, AgentSamplingBoundaryRequest};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct WorkflowSendInvocation {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub tool_call_id: String,
    pub execution_version: String,
    pub outputs: Vec<WorkflowSendOutput>,
}

pub trait WorkflowRuntimeHost: Send + Sync {
    fn snapshot(&self) -> AgentResult<Option<WorkflowConversationSnapshot>>;
    fn send(&self, invocation: WorkflowSendInvocation) -> AgentResult<WorkflowSendReceipt>;
    /// Read projections are scoped to the Host-bound independent conversation and admitted run.
    /// They never claim inputs, acknowledge deliveries or wake a recipient.
    fn state(&self, query: StateQuery) -> AgentResult<Value>;
    fn mailbox(&self, query: MailboxQuery) -> AgentResult<Value>;
    /// Live, compact observation refreshed at each normal model sampling boundary. Unlike the
    /// admitted identity, this must not be persisted in the frozen run snapshot.
    fn awareness(&self) -> AgentResult<Value>;
}

/// A complete input already claimed durably for this conversation, run and sampling boundary.
/// The content is assembled by the Host from verified sender records and the target snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorkflowDelivery {
    pub trace_sequence: u64,
    pub input_id: String,
    pub instance_id: String,
    pub workflow_name: String,
    pub content: String,
    pub created_at: i64,
}

pub trait AgentWorkflowInbox: Send + Sync {
    fn bind_for_model_batch(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<AgentWorkflowDelivery>>;
}
