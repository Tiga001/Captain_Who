use super::super::migrations;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForkDataPolicy {
    CopyVisibleHistory,
    Reinitialize,
    DoNotCopy,
    RuntimeOnly,
    DedicatedForkLogic,
}

const COPY_VISIBLE_HISTORY_TABLES: &[&str] = &[
    "agent_file_change_chunks",
    "agent_file_change_operations",
    "agent_file_changes",
    "agent_run_guidance_attachments",
    "agent_run_guidances",
    "agent_turn_diff_actions",
    "agent_turn_diff_files",
    "agent_turn_diffs",
    "attachments",
    "chat_message_ui_states",
    "context_compaction_receipts",
    "context_compaction_summaries",
    "context_compaction_summary_lineage",
    "conversation_history_blob_chunks",
    "conversation_history_blobs",
    "conversation_model_context_items",
    "conversation_turn_trace_items",
    "conversation_turn_traces",
    "conversation_world_state_records",
    "messages",
];

const REINITIALIZE_TABLES: &[&str] = &[
    "agent_collaboration_event_sequences",
    "agent_collaboration_events",
    "composer_drafts",
    "conversation_context_compaction_heads",
    "conversation_history_fts",
    "conversation_world_state_epochs",
];

const DO_NOT_COPY_TABLES: &[&str] = &[
    "agent_usage_records",
    "automation_events",
    "automation_notification_outbox",
    "automation_runs",
    "automations",
    "browser_downloads",
    "notification_batch_items",
    "notification_events",
];

const RUNTIME_ONLY_TABLES: &[&str] = &[
    "agent_collaboration_cursors",
    "agent_command_session_lifecycle_events",
    "agent_command_session_model_read_receipts",
    "agent_command_session_output_chunks",
    "agent_command_session_published_outputs",
    "agent_command_sessions",
    "agent_effective_permission_snapshots",
    "agent_file_change_run_grants",
    "agent_interrupt_requests",
    "agent_tree_run_stop_members",
    "agent_tree_run_stops",
    "agent_model_batch_receipt_items",
    "agent_model_batch_receipt_replays",
    "agent_model_batch_receipt_targets",
    "agent_model_batch_receipts",
    "agent_pending_actions",
    "agent_wake_requests",
];

const DEDICATED_FORK_LOGIC_TABLES: &[&str] = &[
    "agent_action_audit",
    "agent_mailbox_messages",
    "agent_member_conversation_forks",
    "agent_nodes",
    "child_context_snapshots",
    "conversation_context_adaptation_requirements",
    "conversation_forks",
    "conversation_turn_rewrites",
    "conversations",
    "managed_artifact_grants",
    "model_request_observations",
    "provider_continuation_tool_calls",
    "provider_continuations",
    "provider_transition_terminal_records",
];

fn declared_policies() -> BTreeMap<&'static str, ForkDataPolicy> {
    let mut policies = BTreeMap::new();
    for (policy, tables) in [
        (
            ForkDataPolicy::CopyVisibleHistory,
            COPY_VISIBLE_HISTORY_TABLES,
        ),
        (ForkDataPolicy::Reinitialize, REINITIALIZE_TABLES),
        (ForkDataPolicy::DoNotCopy, DO_NOT_COPY_TABLES),
        (ForkDataPolicy::RuntimeOnly, RUNTIME_ONLY_TABLES),
        (
            ForkDataPolicy::DedicatedForkLogic,
            DEDICATED_FORK_LOGIC_TABLES,
        ),
    ] {
        for table in tables {
            assert!(
                policies.insert(*table, policy).is_none(),
                "Fork table policy is declared more than once: {table}"
            );
        }
    }
    policies
}

fn discover_fork_related_tables(connection: &Connection) -> rusqlite::Result<BTreeSet<String>> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE fork_related(name) AS (
             SELECT 'conversations'
             UNION SELECT 'composer_drafts'
             UNION
             SELECT DISTINCT schema.name
             FROM sqlite_schema AS schema
             JOIN pragma_table_info(schema.name) AS column
             WHERE schema.type = 'table'
               AND schema.name NOT LIKE 'sqlite_%'
               AND (
                    column.name LIKE '%conversation_id%'
                    OR column.name LIKE '%message_id%'
                    OR column.name LIKE '%run_id%'
                    OR column.name LIKE '%agent_id%'
               )
             UNION
             SELECT DISTINCT schema.name
             FROM sqlite_schema AS schema
             JOIN pragma_foreign_key_list(schema.name) AS foreign_key
             JOIN fork_related AS parent ON foreign_key.[table] = parent.name
             WHERE schema.type = 'table'
               AND schema.name NOT LIKE 'sqlite_%'
         )
         SELECT name FROM fork_related ORDER BY name",
    )?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect();
    tables
}

#[test]
fn every_fork_related_table_has_an_explicit_policy() {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();

    let discovered = discover_fork_related_tables(&connection).unwrap();
    let declared = declared_policies()
        .keys()
        .map(|table| (*table).to_string())
        .collect::<BTreeSet<_>>();

    let missing = discovered.difference(&declared).collect::<Vec<_>>();
    let stale = declared.difference(&discovered).collect::<Vec<_>>();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "Fork table policy registry is out of sync. Missing: {missing:?}; stale: {stale:?}"
    );
}

#[test]
fn high_risk_fork_policies_stay_explicit() {
    let policies = declared_policies();
    assert_eq!(
        policies.get("composer_drafts"),
        Some(&ForkDataPolicy::Reinitialize)
    );
    assert_eq!(
        policies.get("chat_message_ui_states"),
        Some(&ForkDataPolicy::CopyVisibleHistory)
    );
    assert_eq!(
        policies.get("agent_action_audit"),
        Some(&ForkDataPolicy::DedicatedForkLogic)
    );
    assert_eq!(
        policies.get("managed_artifact_grants"),
        Some(&ForkDataPolicy::DedicatedForkLogic)
    );
    assert_eq!(
        policies.get("agent_command_sessions"),
        Some(&ForkDataPolicy::RuntimeOnly)
    );
    assert_eq!(
        policies.get("agent_usage_records"),
        Some(&ForkDataPolicy::DoNotCopy)
    );
}
