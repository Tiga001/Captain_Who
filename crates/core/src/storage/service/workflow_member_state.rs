use super::StorageService;
use rusqlite::Connection;
use std::collections::BTreeMap;

/// Durable facts for explicit organization state queries. A missing map entry means the
/// conversation binding no longer exists; occupancy alone does not prove a resident worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowMemberRuntimeState {
    pub active_run_id: Option<String>,
    pub waiting_for_interaction: bool,
}

impl StorageService {
    pub fn load_workflow_member_runtime_states(
        &self,
        fallback_runs: &BTreeMap<String, Option<String>>,
    ) -> Result<BTreeMap<String, WorkflowMemberRuntimeState>, String> {
        if fallback_runs.is_empty() {
            return Ok(BTreeMap::new());
        }
        let selected = serde_json::to_string(fallback_runs).map_err(|error| error.to_string())?;
        read_states(&*self.state.connection()?, &selected).map_err(super::storage_error)
    }
}

fn read_states(
    connection: &Connection,
    selected: &str,
) -> rusqlite::Result<BTreeMap<String, WorkflowMemberRuntimeState>> {
    // One statement observes all durable facts at the same SQLite cut. The caller has already
    // released its short-lived memory locks. Preserve durable > occupancy > resident precedence,
    // and the exact sync-wait predicate (executing/model_in_flight are not waiting for input).
    let mut statement = connection.prepare(
        "SELECT conversation.id,COALESCE(trace.run_id,selected.value),
                EXISTS(SELECT 1 FROM human_interaction_suspensions AS suspension
                       WHERE suspension.run_id=COALESCE(trace.run_id,selected.value)
                         AND suspension.status IN ('waiting','claimed'))
         FROM json_each(?1) AS selected
         JOIN conversations AS conversation ON conversation.id=selected.key
         LEFT JOIN conversation_turn_traces AS trace
           ON trace.conversation_id=conversation.id AND trace.terminal_status='in_progress'
          AND NOT EXISTS(SELECT 1 FROM conversation_turn_rewrites AS rewrite
                         WHERE rewrite.conversation_id=trace.conversation_id
                           AND rewrite.source_assistant_message_id=trace.assistant_message_id)",
    )?;
    let states = statement
        .query_map([selected], |row| {
            Ok((
                row.get(0)?,
                WorkflowMemberRuntimeState {
                    active_run_id: row.get(1)?,
                    waiting_for_interaction: row.get(2)?,
                },
            ))
        })?
        .collect();
    states
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{conversation_trace_repository, migrations};
    use crate::ConversationTraceSnapshot;

    thread_local! {
        static QUERY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn count_query(_: &str) {
        QUERY_COUNT.with(|count| count.set(count.get() + 1));
    }

    #[test]
    fn explicit_member_state_queries_are_bounded_and_preserve_durable_precedence() {
        for count in [1, 128] {
            let mut c = Connection::open_in_memory().unwrap();
            migrations::run_migrations(&c).unwrap();
            let mut selected = BTreeMap::new();
            for index in 0..count {
                let chat = format!("chat-{index}");
                let assistant = format!("assistant-{index}");
                c.execute(
                    "INSERT INTO conversations(id,title,created_at,updated_at) VALUES(?1,'',1,1)",
                    [&chat],
                )
                .unwrap();
                c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES(?1,?2,'assistant','',1,0)", rusqlite::params![assistant,chat]).unwrap();
                selected.insert(chat.clone(), Some(format!("fallback-{index}")));
                if index == 0 {
                    let trace = ConversationTraceSnapshot::default().in_progress_trace(
                        "durable-run",
                        &chat,
                        &assistant,
                    );
                    conversation_trace_repository::append_in_progress_trace(&mut c, &trace, 1, 1)
                        .unwrap();
                }
            }
            selected.insert("deleted".into(), Some("retired-run".into()));
            QUERY_COUNT.with(|count| count.set(0));
            c.trace(Some(count_query));
            let rows = read_states(&c, &serde_json::to_string(&selected).unwrap()).unwrap();
            c.trace(None);
            assert_eq!(QUERY_COUNT.with(|count| count.get()), 1);
            assert_eq!(rows.len(), count);
            assert_eq!(rows["chat-0"].active_run_id.as_deref(), Some("durable-run"));
            for index in 1..count {
                assert_eq!(
                    rows[&format!("chat-{index}")].active_run_id,
                    Some(format!("fallback-{index}"))
                );
            }
            assert!(rows.values().all(|row| !row.waiting_for_interaction));
        }
    }
}
