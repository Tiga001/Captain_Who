use super::*;
use rusqlite::params;

fn initialize(c: &Connection) {
    c.execute_batch(
        "CREATE TABLE workflow_mail_runs(run_id TEXT, conversation_id TEXT, snapshot_json TEXT);
         CREATE TABLE conversation_turn_traces(
             run_id TEXT, conversation_id TEXT, assistant_message_id TEXT,
             created_at INTEGER, completed_at INTEGER, updated_at INTEGER);
         CREATE TABLE agent_nodes(
             agent_id TEXT, parent_agent_id TEXT, root_agent_id TEXT, conversation_id TEXT);
         CREATE TABLE agent_wake_requests(
             agent_id TEXT, root_agent_id TEXT, run_id TEXT, assistant_message_id TEXT,
             source_agent_message_id TEXT, result_message_id TEXT, requester_agent_id TEXT);
         CREATE TABLE agent_mailbox_messages(
             message_id TEXT, kind TEXT, sender_agent_id TEXT, recipient_agent_id TEXT,
             root_agent_id TEXT);
         CREATE TABLE agent_collaboration_events(
             activity_anchor_message_id TEXT, activity_parent_conversation_id TEXT,
             kind TEXT, activity_semantic TEXT, message_id TEXT, agent_id TEXT,
             root_agent_id TEXT, activity_parent_agent_id TEXT);",
    )
    .unwrap();
}

fn setup() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    initialize(&c);
    c
}

fn trace(c: &Connection, run: &str, chat: &str, start: i64, end: Option<i64>) {
    c.execute(
        "INSERT INTO conversation_turn_traces VALUES (?1,?2,?1,?3,?4,?3)",
        params![run, chat, start, end],
    )
    .unwrap();
}

fn admit(c: &Connection, instance: &str, run: &str, chat: &str) {
    c.execute(
        "INSERT INTO workflow_mail_runs VALUES (?1,?2,?3)",
        params![
            run,
            chat,
            serde_json::json!({"instanceId":instance,"nodeId":chat}).to_string()
        ],
    )
    .unwrap();
}

fn current(c: &Connection) -> Option<Activity> {
    latest_for_instances(c, &["org"]).unwrap().remove("org")
}

fn activity(started_at: i64, completed_at: Option<i64>) -> Option<Activity> {
    Some(Activity {
        started_at,
        completed_at,
    })
}

#[test]
fn organization_activity_unions_parallel_bridges_and_restarts_after_idle() {
    let c = setup();
    for (run, start, end) in [
        ("wide", 10, 100),
        ("nested", 20, 30),
        ("bridge", 50, 150),
        ("touching", 150, 170),
    ] {
        trace(&c, run, run, start, Some(end));
        admit(&c, "org", run, run);
    }
    // The short nested run must not break the interval covered by the longer running member.
    assert_eq!(current(&c), activity(10, Some(170)));
    trace(&c, "later", "wide", 200, Some(230));
    admit(&c, "org", "later", "wide");
    assert_eq!(current(&c), activity(200, Some(230)));
}

#[test]
fn organization_activity_tracks_the_last_active_turn_and_freezes_at_its_end() {
    let c = setup();
    trace(&c, "a", "a", 10, Some(30));
    trace(&c, "b", "b", 20, None);
    admit(&c, "org", "a", "a");
    admit(&c, "org", "b", "b");
    assert_eq!(current(&c), activity(10, None));
    c.execute(
        "UPDATE conversation_turn_traces SET completed_at=50,updated_at=5000 WHERE run_id='b'",
        [],
    )
    .unwrap();
    assert_eq!(current(&c), activity(10, Some(50)));
}

#[test]
fn organization_activity_excludes_unadmitted_history_failed_admissions_and_wrong_owners() {
    let c = setup();
    trace(&c, "before-binding", "a", 1, Some(100));
    admit(&c, "org", "never-started", "a");
    trace(&c, "wrong-owner", "other", 5, None);
    admit(&c, "org", "wrong-owner", "a");
    trace(&c, "disabled", "a", 20, None);
    c.execute(
        "INSERT INTO workflow_mail_runs VALUES ('disabled','a','null')",
        [],
    )
    .unwrap();
    assert_eq!(current(&c), None);
    trace(&c, "actual", "a", 200, Some(250));
    admit(&c, "org", "actual", "a");
    assert_eq!(current(&c), activity(200, Some(250)));
}

#[test]
fn organization_activity_batches_organizations_and_ignores_metadata_changes() {
    let c = setup();
    trace(&c, "a", "a", 10, Some(50));
    admit(&c, "org", "a", "a");
    trace(&c, "b", "b", 20, None);
    admit(&c, "other", "b", "b");
    c.execute(
        "UPDATE workflow_mail_runs SET snapshot_json=json_set(snapshot_json,
             '$.executionVersion','new','$.organizationRevision',9,'$.name','Renamed')",
        [],
    )
    .unwrap();
    let results = latest_for_instances(&c, &["org", "other", "empty"]).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results.get("org"), activity(10, Some(50)).as_ref());
    assert_eq!(results.get("other"), activity(20, None).as_ref());
    assert_eq!(current(&c), activity(10, Some(50)));
}

fn node(c: &Connection, id: &str, parent: Option<&str>) {
    c.execute(
        "INSERT INTO agent_nodes VALUES (?1,?2,'root',?1)",
        params![id, parent],
    )
    .unwrap();
}

fn delegate(c: &Connection, parent: &str, parent_run: &str, child: &str, child_run: &str) {
    c.execute(
        "INSERT INTO agent_wake_requests VALUES (?1,'root',?2,?2,?2,NULL,?3)",
        params![child, child_run, parent],
    )
    .unwrap();
    c.execute(
        "INSERT INTO agent_collaboration_events VALUES
             (?1,?2,'wake_created','started',?4,?3,'root',?2)",
        params![parent_run, parent, child, child_run],
    )
    .unwrap();
}

#[test]
fn organization_activity_includes_proven_descendants_that_outlive_the_root() {
    let c = setup();
    node(&c, "root", None);
    node(&c, "child", Some("root"));
    node(&c, "grandchild", Some("child"));
    trace(&c, "root-run", "root", 10, Some(30));
    admit(&c, "org", "root-run", "root");
    trace(&c, "child-run", "child", 20, Some(50));
    delegate(&c, "root", "root-run", "child", "child-run");
    trace(&c, "grandchild-run", "grandchild", 40, None);
    delegate(&c, "child", "child-run", "grandchild", "grandchild-run");
    // Being in the same tree is insufficient: neither a pre-binding nor an unanchored Turn counts.
    trace(&c, "old-root", "root", 1, Some(8));
    trace(&c, "old-child", "child", 2, Some(500));
    delegate(&c, "root", "old-root", "child", "old-child");
    trace(&c, "unproven-child", "child", 200, None);
    assert_eq!(current(&c), activity(10, None));
    c.execute(
        "UPDATE conversation_turn_traces SET completed_at=80 WHERE run_id='grandchild-run'",
        [],
    )
    .unwrap();
    assert_eq!(current(&c), activity(10, Some(80)));
}

#[test]
fn organization_activity_follows_result_wakes_without_inventing_idle_work() {
    let c = setup();
    node(&c, "root", None);
    node(&c, "child", Some("root"));
    node(&c, "grandchild", Some("child"));
    trace(&c, "root-run", "root", 10, Some(30));
    admit(&c, "org", "root-run", "root");
    trace(&c, "child-run", "child", 20, Some(50));
    delegate(&c, "root", "root-run", "child", "child-run");
    trace(&c, "grandchild-run", "grandchild", 40, Some(80));
    delegate(&c, "child", "child-run", "grandchild", "grandchild-run");
    c.execute(
        "UPDATE agent_wake_requests SET result_message_id='result' WHERE run_id='grandchild-run'",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO agent_mailbox_messages VALUES ('result','result','grandchild','child','root')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO agent_wake_requests VALUES ('child','root','followup','followup','result',NULL,'grandchild')",
        [],
    )
    .unwrap();
    trace(&c, "followup", "child", 90, None);
    // The durable result gives ownership, not execution during the idle gap from 80 to 90.
    assert_eq!(current(&c), activity(90, None));
}

#[test]
fn organization_activity_does_not_assign_an_ancestor_followup_to_an_old_parent_run() {
    let c = setup();
    node(&c, "root", None);
    node(&c, "child", Some("root"));
    node(&c, "grandchild", Some("child"));
    trace(&c, "root-old", "root", 10, Some(30));
    admit(&c, "org", "root-old", "root");
    trace(&c, "child-old", "child", 20, Some(50));
    delegate(&c, "root", "root-old", "child", "child-old");
    trace(&c, "root-new", "root", 100, Some(150));
    admit(&c, "other", "root-new", "root");
    trace(&c, "grandchild-new", "grandchild", 110, None);
    // UI placement follows the direct parent, while an ancestor actually dispatched this work.
    delegate(&c, "child", "child-old", "grandchild", "grandchild-new");
    c.execute(
        "UPDATE agent_wake_requests SET requester_agent_id='root' WHERE run_id='grandchild-new'",
        [],
    )
    .unwrap();
    assert_eq!(current(&c), activity(10, Some(50)));
    let other = latest_for_instances(&c, &["other"]).unwrap();
    assert_eq!(other.get("other"), activity(100, Some(150)).as_ref());
}

#[test]
fn organization_activity_is_restored_on_reopen_without_accumulating_parallel_time() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("activity.sqlite");
    {
        let c = Connection::open(&path).unwrap();
        initialize(&c);
        trace(&c, "a", "a", 100, Some(200));
        trace(&c, "b", "b", 150, None);
        admit(&c, "org", "a", "a");
        admit(&c, "org", "b", "b");
    }
    let c = Connection::open(&path).unwrap();
    assert_eq!(current(&c), activity(100, None));
    c.execute(
        "UPDATE conversation_turn_traces SET completed_at=250 WHERE run_id='b'",
        [],
    )
    .unwrap();
    drop(c);
    assert_eq!(
        current(&Connection::open(&path).unwrap()),
        activity(100, Some(250))
    );
}
