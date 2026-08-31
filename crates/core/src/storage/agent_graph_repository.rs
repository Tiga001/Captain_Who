//! Transactional persistence for the deliberately small Agent parent-child tree.
//!
//! SQLite is the coordination source of truth. Runtime notifications may observe these records,
//! but no in-memory queue is allowed to substitute for them.

mod common;
mod mailbox;
mod message_records;
mod node_records;
mod nodes;
mod permissions;
mod settlement;
mod wake_commands;
mod wake_projection;
mod wake_records;
mod wake_resolution;
mod wakes;

pub use mailbox::{
    acknowledge_agent_message_with_projection, acknowledge_agent_task_with_projection_and_wake,
    claim_next_agent_message, conversation_message_origin, conversation_message_origins,
    enqueue_agent_message, follow_up_agent, get_agent_message, renew_agent_message_lease,
    send_agent_message,
};
pub(crate) use mailbox::{
    create_initial_agent_task_and_wake_in_transaction,
    project_pending_agent_messages_in_transaction,
    satisfy_agent_wake_by_source_message_in_transaction,
};
#[cfg(test)]
pub(crate) use nodes::create_agent_node;
pub(crate) use nodes::{
    create_agent_node_in_transaction, ensure_root_agent_in_transaction,
    insert_forked_agent_node_in_transaction,
};
pub use nodes::{
    ensure_conversation_unbound, ensure_project_unbound, ensure_root_agent,
    get_agent_display_status, get_agent_node, get_agent_node_by_conversation, list_agent_children,
    list_agent_tree, transition_agent_lifecycle,
};
pub use permissions::{
    get_agent_effective_permission_snapshot, record_agent_effective_permissions_for_active_turn,
};
pub(crate) use permissions::{
    inherit_agent_permissions_in_transaction, record_agent_effective_permissions_in_transaction,
};
pub use settlement::{finish_agent_turn_with_result, finish_agent_wake_with_result};
pub(crate) use wake_projection::project_agent_wake_source_in_transaction;
pub(crate) use wake_resolution::{
    resolve_active_child_wake_bundle_by_identity, resolve_child_spawn_by_creation_request,
    resolve_child_wake_bundle, resolve_claimed_agent_wake_bundle,
    resolve_running_child_wake_bundle,
};
pub(crate) use wakes::transition_agent_wake_in_connection;
pub use wakes::{
    claim_next_agent_wake, claim_next_dispatchable_agent_wake, enqueue_agent_wake, get_agent_wake,
    interrupt_agent_execution, recover_agent_wakes, renew_agent_wake_lease, transition_agent_wake,
};

#[cfg(test)]
mod tests;
