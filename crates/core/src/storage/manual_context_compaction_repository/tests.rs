use super::*;
use crate::storage::{migrations, usage_repository};
use crate::{AgentUsageClearInput, AgentUsageSummaryInput, AgentUsageSummaryRange};

fn setup() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection.execute("INSERT INTO conversations (id,title,created_at,updated_at) VALUES ('conversation','History',1,1)", []).unwrap();
    connection
}

fn operation(id: &str) -> ManualContextCompactionOperation {
    ManualContextCompactionOperation {
        operation_id: id.into(),
        request_id: id.into(),
        conversation_id: "conversation".into(),
        status: "running".into(),
        phase: "preparing".into(),
        assistant_message_id: None,
        covered_through_message_id: None,
        model_id: None,
        summary_id: None,
        source_input_tokens: None,
        replacement_input_tokens: None,
        error: None,
        started_at: 10,
        updated_at: 10,
        completed_at: None,
    }
}

fn usage(id: &str) -> ManualContextCompactionUsageRecord {
    ManualContextCompactionUsageRecord {
        operation_id: id.into(),
        conversation_id: "conversation".into(),
        project_id: None,
        model_id: "model".into(),
        model_name: "Model".into(),
        started_at: Some(10),
        completed_at: Some(20),
        status: Some("completed".into()),
        error: None,
        created_at: 20,
        input_tokens: Some(100),
        output_tokens: Some(20),
        output_thinking_tokens: None,
        total_tokens: Some(120),
        cached_input_tokens: Some(10),
        cache_creation_input_tokens: None,
        billable_request_count: 1,
        input_price: Some("1".into()),
        cached_input_price: Some("0.1".into()),
        output_price: Some("2".into()),
        estimated_cost: Some(0.131),
    }
}

fn totals(connection: &Connection) -> crate::AgentUsageSummaryOutput {
    usage_repository::usage_summary(
        connection,
        &AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        },
        100,
    )
    .unwrap()
}

#[test]
fn durable_claim_is_idempotent_conflicts_and_terminal_states_never_regress() {
    let mut connection = setup();
    let mut first = claim(&mut connection, &operation("first")).unwrap();
    assert_eq!(claim(&mut connection, &first).unwrap(), first);
    assert!(claim(&mut connection, &operation("second")).is_err());
    let mut mismatch = first.clone();
    mismatch.operation_id = "different".into();
    assert!(claim(&mut connection, &mismatch).is_err());
    first.status = "cancelled".into();
    first.phase = "generating".into();
    first.updated_at = 20;
    first.completed_at = Some(20);
    assert_eq!(update(&connection, &first).unwrap(), first);
    let mut late = first.clone();
    late.status = "completed".into();
    late.summary_id = Some("late-summary".into());
    late.updated_at = 30;
    assert_eq!(update(&connection, &late).unwrap(), first);
    assert_eq!(claim(&mut connection, &operation("first")).unwrap(), first);
    assert!(claim(&mut connection, &operation("second")).is_ok());
}

#[test]
fn restart_interrupts_running_without_replaying_or_losing_usage() {
    let mut connection = setup();
    claim(&mut connection, &operation("restart")).unwrap();
    record_usage(&connection, &usage("restart")).unwrap();
    interrupt_running(&mut connection, 30).unwrap();
    let recovered = get(&connection, "conversation", Some("restart"))
        .unwrap()
        .unwrap();
    assert_eq!(recovered.status, "interrupted");
    assert_eq!(recovered.completed_at, Some(30));
    assert_eq!(
        claim(&mut connection, &operation("restart")).unwrap(),
        recovered
    );
    assert_eq!(totals(&connection).request_count, 1);
    assert_eq!(totals(&connection).message_count, 0);
}

#[test]
fn usage_is_owned_by_operation_and_replayed_responses_never_overwrite_or_double_count() {
    let mut connection = setup();
    connection.execute_batch(
        "INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES ('old-assistant','conversation','assistant','original reply','sent',1,0);
         INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,model_name,created_at,input_tokens,billable_request_count) VALUES ('old-usage','conversation','old-assistant','old-run','model','Model',1,1000,1);"
    ).unwrap();
    claim(&mut connection, &operation("usage")).unwrap();
    let record = usage("usage");
    record_usage(&connection, &record).unwrap();
    record_usage(&connection, &record).unwrap();
    let mut changed = record.clone();
    changed.input_tokens = Some(101);
    assert!(record_usage(&connection, &changed).is_err());
    let result = totals(&connection);
    assert_eq!(result.request_count, 2);
    assert_eq!(result.message_count, 1);
    assert_eq!(result.input_tokens, Some(1100));
    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].message_count, 1);
    assert_eq!(
        connection
            .query_row(
                "SELECT input_tokens FROM agent_usage_records WHERE id='old-usage'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1000
    );
    let cleared = usage_repository::clear_usage_records(
        &connection,
        &AgentUsageClearInput {
            from: None,
            to: None,
        },
    )
    .unwrap();
    assert_eq!(cleared.deleted_records, 2);
    record_usage(&connection, &record).unwrap();
    assert_eq!(totals(&connection).request_count, 0);
}

#[test]
fn deleting_conversation_rolls_up_manual_usage_without_creating_message_counts() {
    let mut connection = setup();
    claim(&mut connection, &operation("delete")).unwrap();
    record_usage(&connection, &usage("delete")).unwrap();
    let before = totals(&connection);
    usage_repository::roll_up_deleted_usage_for_conversation(&connection, "conversation", 100)
        .unwrap();
    connection
        .execute("DELETE FROM conversations WHERE id='conversation'", [])
        .unwrap();
    let after = totals(&connection);
    assert_eq!(after.request_count, before.request_count);
    assert_eq!(after.message_count, 0);
    assert_eq!(after.input_tokens, before.input_tokens);
    assert_eq!(after.estimated_cost, before.estimated_cost);
    assert_eq!(after.models[0].request_count, 1);
}

#[test]
fn missing_provider_usage_stays_unknown() {
    let mut connection = setup();
    claim(&mut connection, &operation("unknown")).unwrap();
    let mut record = usage("unknown");
    record.input_tokens = None;
    record.output_tokens = None;
    record.total_tokens = None;
    record.cached_input_tokens = None;
    record.estimated_cost = None;
    record_usage(&connection, &record).unwrap();
    let result = totals(&connection);
    assert_eq!(result.request_count, 1);
    assert_eq!(result.input_tokens, None);
    assert_eq!(result.estimated_cost, None);
}
