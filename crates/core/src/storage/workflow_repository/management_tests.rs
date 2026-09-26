use super::*;
use crate::storage::migrations::run_migrations;
use serde_json::{json, Value};

fn call(c: &mut Connection, value: Value) -> Result<Response, Error> {
    super::request(
        c,
        serde_json::from_value(value).unwrap(),
        &HashSet::from(["model-default".into()]),
    )
}
fn definition() -> Value {
    json!({"schemaVersion":1,"id":"template","name":"Template","description":"","background":"","nodes":[{"kind":"agent","id":"a","name":"Worker","x":300,"y":100,"permissionMode":"full","modelConfigId":"model-default","receives":"Task","task":"Work","delivers":"Result"}],"flows":[{"id":"entry","name":"S1","source":{"kind":"boundary"},"target":{"kind":"node","nodeId":"a"}}],"viewport":{"x":0,"y":0,"zoom":1},"boundaryPositions":{"input":{"x":0,"y":100}}})
}
fn setup() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    call(
        &mut c,
        json!({"operation":"save","definition":definition(),"expectedRevision":0}),
    )
    .unwrap();
    call(
        &mut c,
        json!({"operation":"save","definition":definition(),"expectedRevision":1}),
    )
    .unwrap();
    c
}
fn create(id: &str, conversation: Option<&str>) -> Value {
    json!({"operation":"saveInstance","id":id,"templateId":"template","name":id,"color":"#4A82E8","bindings":[{"nodeId":"a","conversationId":conversation}],"expectedRevision":0,"expectedTemplateRevision":2})
}
fn existing(c: &Connection, id: &str, project: &str) {
    c.execute(
        "INSERT OR IGNORE INTO projects(id,name,created_at,updated_at) VALUES (?1,?1,1,1)",
        [project],
    )
    .unwrap();
    c.execute("INSERT INTO conversations(id,project_id,model_id,title,created_at,updated_at) VALUES (?1,?2,'existing-model',?1,1,1)",params![id,project]).unwrap();
}
fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn toggle(c: &mut Connection, id: &str, enabled: bool, revision: u64) -> Result<Response, Error> {
    call(
        c,
        json!({"operation":"setInstanceEnabled","id":id,"enabled":enabled,"expectedRevision":revision}),
    )
}

#[test]
fn workflow_disable_preserves_busy_turns_and_confirmed_reconfiguration_reactivates() {
    let mut c = setup();
    let first = call(&mut c, create("instance", None)).unwrap();
    assert!(first.instances[0].enabled);
    let chat = first.instances[0].bindings[0].conversation_id.clone();
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES ('busy',?1,'assistant','Working',1,0)",[&chat]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES ('busy',?1,'run',1,'in_progress',0,1,1)",[&chat]).unwrap();
    assert!(call(&mut c, json!({"operation":"list"})).unwrap().instances[0].running);
    let off = toggle(&mut c, "instance", false, 1).unwrap();
    assert!(!off.instances[0].enabled);
    assert!(!off.instances[0].running);
    assert_eq!(off.instances[0].bindings, first.instances[0].bindings);
    assert_eq!(
        toggle(&mut c, "instance", false, 1).unwrap().instances[0].revision,
        2
    );
    let state: String = c
        .query_row(
            "SELECT terminal_status FROM conversation_turn_traces WHERE run_id='run'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "in_progress");
    let mut configure = create("instance", Some(&chat));
    configure["expectedRevision"] = json!(2);
    let configured = call(&mut c, configure).unwrap();
    assert!(configured.instances[0].enabled && configured.instances[0].running);
    assert!(configured.affected_conversation_ids.is_empty());
}

#[test]
fn workflow_only_enabled_instances_reserve_colors_and_enabling_rechecks_occupancy() {
    let mut c = setup();
    let first = call(&mut c, create("first", None)).unwrap();
    toggle(&mut c, "first", false, 1).unwrap();
    call(&mut c, create("second", None)).unwrap();
    assert!(
        matches!(toggle(&mut c,"first",true,2),Err(Error::Conflict(message)) if message=="workflow_color_in_use")
    );
    let mut configure = create(
        "first",
        Some(&first.instances[0].bindings[0].conversation_id),
    );
    configure["expectedRevision"] = json!(2);
    assert!(
        matches!(call(&mut c, configure.clone()),Err(Error::Conflict(message)) if message=="workflow_color_in_use")
    );
    toggle(&mut c, "second", false, 1).unwrap();
    let confirmed = call(&mut c, configure).unwrap();
    assert!(
        confirmed
            .instances
            .iter()
            .find(|instance| instance.id == "first")
            .unwrap()
            .enabled
    );
}

#[test]
fn workflow_template_updates_disable_instances_until_binding_confirmation() {
    let mut c = setup();
    let first = call(&mut c, create("instance", None)).unwrap();
    let published=call(&mut c,json!({"operation":"save","definition":definition(),"expectedRevision":2,"expectedUsageRevision":first.usages[0].usage_revision})).unwrap();
    assert!(!published.instances[0].enabled && published.instances[0].needs_review);
    assert!(
        matches!(toggle(&mut c,"instance",true,2),Err(Error::Invalid(message)) if message=="workflow_instance_needs_review")
    );
    let mut configure = create(
        "instance",
        Some(&first.instances[0].bindings[0].conversation_id),
    );
    configure["expectedRevision"] = json!(2);
    configure["expectedTemplateRevision"] = json!(3);
    let reviewed = call(&mut c, configure).unwrap();
    assert!(reviewed.instances[0].enabled && !reviewed.instances[0].needs_review);
    toggle(&mut c, "instance", false, 3).unwrap();
    c.execute(
        "DELETE FROM workflow_instance_bindings WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    assert!(
        matches!(toggle(&mut c,"instance",true,4),Err(Error::Invalid(message)) if message=="workflow_bindings_incomplete")
    );
}

#[test]
fn workflow_archive_guard_covers_metadata_full_save_and_atomic_multirow_updates() {
    let mut c = setup();
    existing(&c, "a-unbound", "P");
    existing(&c, "z-bound", "P");
    call(&mut c, create("instance", Some("z-bound"))).unwrap();
    let mut full = crate::storage::chat_repository::get_conversation(&c, "z-bound")
        .unwrap()
        .unwrap();
    full.archived_at = Some(10);
    full.title = "Should not persist".into();
    assert!(
        crate::storage::chat_repository::save_conversation(&mut c, full)
            .unwrap_err()
            .to_string()
            .contains("workflow_active_archive_blocked")
    );
    let mut meta = crate::storage::chat_repository::list_conversation_metas(&c)
        .unwrap()
        .into_iter()
        .find(|chat| chat.id == "z-bound")
        .unwrap();
    meta.archived_at = Some(10);
    meta.updated_at = 10;
    assert!(
        crate::storage::chat_repository::save_conversation_meta(&c, &meta)
            .unwrap_err()
            .to_string()
            .contains("workflow_active_archive_blocked")
    );
    assert!(c
        .execute("UPDATE conversations SET archived_at=10", [])
        .unwrap_err()
        .to_string()
        .contains("workflow_active_archive_blocked"));
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM conversations WHERE archived_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        crate::storage::chat_repository::get_conversation(&c, "z-bound")
            .unwrap()
            .unwrap()
            .title,
        "z-bound"
    );
    toggle(&mut c, "instance", false, 1).unwrap();
    c.execute("UPDATE conversations SET archived_at=10", [])
        .unwrap();
    let after = call(&mut c, json!({"operation":"list"})).unwrap();
    assert!(!after.instances[0].enabled && after.instances[0].needs_review);
    assert_eq!(count(&c, "conversations"), 2);
    assert_eq!(after.instances[0].bindings.len(), 1);
}

#[test]
fn workflow_instances_initialize_once_preserve_draft_and_allow_cross_project_replacement() {
    let mut c = setup();
    existing(&c, "existing", "Project A");
    existing(&c, "replacement", "Project B");
    c.execute("INSERT INTO composer_drafts(scope_id,message,permission_mode,permission_mode_version,model_id,project_id,attachments_json,skills_json,queued_messages_json,updated_at) VALUES ('existing','Unsent text','custom',2,'draft-model','Project A','[]','[]','[]',9)",[]).unwrap();
    let result = call(&mut c, create("instance", Some("existing"))).unwrap();
    assert_eq!(result.affected_conversation_ids, vec!["existing"]);
    let draft:(String,String,String)=c.query_row("SELECT message,model_id,permission_mode FROM composer_drafts WHERE scope_id='existing'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        draft,
        ("Unsent text".into(), "model-default".into(), "full".into())
    );
    let model: String = c
        .query_row(
            "SELECT model_id FROM conversations WHERE id='existing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(model, "existing-model");
    c.execute(
        "UPDATE composer_drafts SET permission_mode='default',model_id='user-chosen-model' WHERE scope_id='existing'",
        [],
    )
    .unwrap();
    let mut update = create("instance", Some("existing"));
    update["expectedRevision"] = json!(1);
    let updated = call(&mut c, update).unwrap();
    assert!(updated.affected_conversation_ids.is_empty());
    let mode: String = c
        .query_row(
            "SELECT permission_mode FROM composer_drafts WHERE scope_id='existing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mode, "default");
    let selected_model: String = c
        .query_row(
            "SELECT model_id FROM composer_drafts WHERE scope_id='existing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(selected_model, "user-chosen-model");
    let mut replace = create("instance", Some("replacement"));
    replace["expectedRevision"] = json!(2);
    let changed = call(&mut c, replace).unwrap();
    assert_eq!(changed.affected_conversation_ids, vec!["replacement"]);
    assert_eq!(
        changed.usages[0].instances[0].project_names,
        vec!["Project B"]
    );
    assert_eq!(count(&c, "conversations"), 2);
}

#[test]
fn workflow_instance_confirmation_is_idempotent_and_removal_keeps_conversations() {
    let mut c = setup();
    let request = create("instance", None);
    let first = call(&mut c, request.clone()).unwrap();
    let conversation = &first.instances[0].bindings[0].conversation_id;
    let project: Option<String> = c
        .query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [conversation],
            |r| r.get(0),
        )
        .unwrap();
    assert!(project.is_none());
    assert_eq!(count(&c, "messages"), 0);
    let repeated = call(&mut c, request).unwrap();
    assert_eq!(count(&c, "conversations"), 1);
    assert_eq!(
        first.affected_conversation_ids,
        repeated.affected_conversation_ids
    );
    call(
        &mut c,
        json!({"operation":"deleteInstance","id":"instance","expectedRevision":1}),
    )
    .unwrap();
    assert_eq!(count(&c, "conversations"), 1);
    assert_eq!(count(&c, "workflow_instance_bindings"), 0);
}

#[test]
fn workflow_colors_are_exclusive_case_insensitively_before_any_conversation_changes() {
    let mut c = setup();
    let first_request = create("first", None);
    let first = call(&mut c, first_request.clone()).unwrap();
    assert!(!first.instances[0].running);
    existing(&c, "existing", "P");
    c.execute("INSERT INTO composer_drafts(scope_id,message,permission_mode,permission_mode_version,model_id,project_id,attachments_json,skills_json,queued_messages_json,updated_at) VALUES ('existing','Unsent text','custom',2,'user-model','P','[]','[]','[]',9)",[]).unwrap();
    let draft_before: String = c.query_row("SELECT json_array(message,permission_mode,model_id,updated_at) FROM composer_drafts WHERE scope_id='existing'",[],|r|r.get(0)).unwrap();
    for conversation in [None, Some("existing")] {
        for color in ["#4A82E8", "#4a82e8"] {
            let mut duplicate = create("second", conversation);
            duplicate["color"] = json!(color);
            assert!(
                matches!(call(&mut c, duplicate),Err(Error::Conflict(message)) if message=="workflow_color_in_use")
            );
            assert_eq!(count(&c, "workflow_instances"), 1);
            assert_eq!(count(&c, "workflow_instance_bindings"), 1);
            assert_eq!(count(&c, "conversations"), 2);
            assert_eq!(count(&c, "composer_drafts"), 2);
            let draft_after: String = c.query_row("SELECT json_array(message,permission_mode,model_id,updated_at) FROM composer_drafts WHERE scope_id='existing'",[],|r|r.get(0)).unwrap();
            assert_eq!(draft_after, draft_before);
        }
    }
    // A successful confirmation remains safely replayable without creating another chat.
    let replayed = call(&mut c, first_request).unwrap();
    assert_eq!(
        replayed.affected_conversation_ids,
        first.affected_conversation_ids
    );
    assert_eq!(count(&c, "conversations"), 2);
}

#[test]
fn workflow_can_keep_its_color_move_to_a_free_color_and_release_it_on_removal() {
    let mut c = setup();
    let created = call(&mut c, create("first", None)).unwrap();
    let conversation_id = &created.instances[0].bindings[0].conversation_id;
    let mut update = create("first", Some(conversation_id));
    update["expectedRevision"] = json!(1);
    update["color"] = json!("#4a82e8");
    let retained = call(&mut c, update.clone()).unwrap();
    assert_eq!(retained.instances[0].color, "#4a82e8");
    assert!(retained.affected_conversation_ids.is_empty());
    update["expectedRevision"] = json!(2);
    update["color"] = json!("#AA0000");
    let changed = call(&mut c, update.clone()).unwrap();
    assert_eq!(changed.instances[0].color, "#AA0000");
    assert!(changed.affected_conversation_ids.is_empty());
    let second = call(&mut c, create("second", None)).unwrap();
    assert_eq!(second.instances.len(), 2);
    // An existing instance also cannot take a color claimed after its editor was opened.
    update["expectedRevision"] = json!(3);
    update["color"] = json!("#4a82e8");
    assert!(
        matches!(call(&mut c, update.clone()),Err(Error::Conflict(message)) if message=="workflow_color_in_use")
    );
    call(
        &mut c,
        json!({"operation":"deleteInstance","id":"second","expectedRevision":1}),
    )
    .unwrap();
    let released = call(&mut c, update).unwrap();
    assert_eq!(released.instances.len(), 1);
    assert_eq!(released.instances[0].color, "#4a82e8");
    assert_eq!(released.instances[0].revision, 4);
    assert_eq!(count(&c, "conversations"), 2);
}

#[test]
fn workflow_concurrent_confirmations_cannot_claim_the_same_color_from_stale_lists() {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("workflow-colors.sqlite");
    let mut first = Connection::open(&path).unwrap();
    run_migrations(&first).unwrap();
    call(
        &mut first,
        json!({"operation":"save","definition":definition(),"expectedRevision":0}),
    )
    .unwrap();
    call(
        &mut first,
        json!({"operation":"save","definition":definition(),"expectedRevision":1}),
    )
    .unwrap();
    let mut second = Connection::open(&path).unwrap();
    for connection in [&mut first, &mut second] {
        connection.busy_timeout(Duration::from_secs(5)).unwrap();
        assert!(call(connection, json!({"operation":"listInstances"}))
            .unwrap()
            .instances
            .is_empty());
    }
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let left_barrier = Arc::clone(&barrier);
        let left = scope.spawn(move || {
            left_barrier.wait();
            call(&mut first, create("first", None))
        });
        let right = scope.spawn(move || {
            barrier.wait();
            let mut request = create("second", None);
            request["color"] = json!("#4a82e8");
            call(&mut second, request)
        });
        [left.join().unwrap(), right.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result|matches!(result,Err(Error::Conflict(message)) if message=="workflow_color_in_use")).count(),1);
    let verified = Connection::open(path).unwrap();
    assert_eq!(count(&verified, "workflow_instances"), 1);
    assert_eq!(count(&verified, "workflow_instance_bindings"), 1);
    assert_eq!(count(&verified, "conversations"), 1);
    assert_eq!(count(&verified, "composer_drafts"), 1);
}

#[test]
fn workflow_bindings_are_exclusive_and_fail_atomically() {
    let mut c = setup();
    existing(&c, "existing", "P");
    call(&mut c, create("first", Some("existing"))).unwrap();
    let mut second = create("second", Some("existing"));
    second["color"] = json!("#00AA00");
    assert!(matches!(
        call(&mut c, second),
        Err(Error::Conflict(message)) if message == "workflow_conversation_already_bound"
    ));
    assert_eq!(count(&c, "workflow_instances"), 1);
    let mut graph = definition();
    let mut other = graph["nodes"][0].clone();
    other["id"] = json!("b");
    graph["nodes"].as_array_mut().unwrap().push(other);
    let mut flow = graph["flows"][0].clone();
    flow["id"] = json!("second");
    flow["target"]["nodeId"] = json!("b");
    graph["flows"].as_array_mut().unwrap().push(flow);
    let usages = call(&mut c, json!({"operation":"list"})).unwrap().usages;
    call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":2,"expectedUsageRevision":usages[0].usage_revision})).unwrap();
    let mut invalid = create("new", None);
    invalid["color"] = json!("#AA0000");
    invalid["expectedTemplateRevision"] = json!(3);
    invalid["bindings"] =
        json!([{"nodeId":"a","conversationId":null},{"nodeId":"b","conversationId":"missing"}]);
    assert!(call(&mut c, invalid).is_err());
    assert_eq!(count(&c, "conversations"), 1);
    assert_eq!(count(&c, "workflow_instances"), 1);
}

#[test]
fn workflow_template_publish_requires_usage_confirmation_and_reconciles_stable_nodes() {
    let mut c = setup();
    let created = call(&mut c, create("instance", None)).unwrap();
    let original = created.instances[0].bindings[0].clone();
    let mut graph = definition();
    graph["nodes"][0]["name"] = json!("Renamed");
    assert!(
        matches!(call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":2})),Err(Error::Conflict(message)) if message=="workflow_usage_changed")
    );
    let updated=call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":2,"expectedUsageRevision":created.usages[0].usage_revision})).unwrap();
    assert_eq!(updated.instances[0].bindings, vec![original]);
    assert!(updated.instances[0].needs_review);
    assert!(call(
        &mut c,
        json!({"operation":"delete","id":"template","expectedRevision":3})
    )
    .is_err());
    graph["nodes"] = json!([]);
    graph["flows"] = json!([]);
    let removed=call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":3,"expectedUsageRevision":updated.usages[0].usage_revision})).unwrap();
    assert!(removed.instances[0].bindings.is_empty());
    assert_eq!(count(&c, "conversations"), 1);
}

#[test]
fn workflow_running_guard_preserves_original_and_supports_isolated_draft_and_copy() {
    let mut c = setup();
    let created = call(&mut c, create("instance", None)).unwrap();
    c.execute(
        "UPDATE workflow_instances SET running=1 WHERE instance_id='instance'",
        [],
    )
    .unwrap();
    let mut graph = definition();
    graph["name"] = json!("Unsaved edit");
    assert!(
        matches!(call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":2,"expectedUsageRevision":created.usages[0].usage_revision})),Err(Error::Conflict(message)) if message=="workflow_template_running")
    );
    let stashed=call(&mut c,json!({"operation":"saveDraft","definition":graph,"expectedRevision":2,"expectedDraftRevision":0})).unwrap();
    assert_eq!(stashed.records[0].definition.name, "Template");
    assert_eq!(stashed.drafts[0].definition.name, "Unsaved edit");
    assert!(call(&mut c,json!({"operation":"saveDraft","definition":graph,"expectedRevision":2,"expectedDraftRevision":0})).is_err());
    let copied=call(&mut c,json!({"operation":"duplicate","id":"template","expectedRevision":2,"newId":"copy","name":"Copy"})).unwrap();
    assert_eq!(copied.records.len(), 2);
    assert_eq!(copied.instances[0].template_id, "template");
    graph["id"] = json!("edited-copy");
    let copy = call(
        &mut c,
        json!({"operation":"save","definition":graph,"expectedRevision":0}),
    )
    .unwrap();
    assert_eq!(copy.records.len(), 3);
    assert!(call(
        &mut c,
        json!({"operation":"deleteInstance","id":"instance","expectedRevision":1})
    )
    .is_err());
}

#[test]
fn workflow_conversation_deletion_marks_instance_for_repair_without_touching_others() {
    let mut c = setup();
    let created = call(&mut c, create("instance", None)).unwrap();
    let id = &created.instances[0].bindings[0].conversation_id;
    c.execute("DELETE FROM conversations WHERE id=?1", [id])
        .unwrap();
    let result = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert!(result.instances[0].needs_review);
    assert!(result.instances[0].bindings.is_empty());
    assert_eq!(result.instances[0].revision, 2);
}

#[test]
fn workflow_running_state_is_derived_from_active_bound_conversation_turns() {
    let mut c = setup();
    let created = call(&mut c, create("instance", None)).unwrap();
    let conversation = &created.instances[0].bindings[0].conversation_id;
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES ('assistant',?1,'assistant','',1,0)",[conversation]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES ('assistant',?1,'run',1,'in_progress',0,1,1)",[conversation]).unwrap();
    let running = call(&mut c, json!({"operation":"list"})).unwrap();
    assert!(running.instances[0].running);
    assert!(running.usages[0].instances[0].running);
    assert!(
        matches!(call(&mut c,json!({"operation":"save","definition":definition(),"expectedRevision":2,"expectedUsageRevision":running.usages[0].usage_revision})),Err(Error::Conflict(message)) if message=="workflow_template_running")
    );
    c.execute("UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=2,updated_at=2 WHERE run_id='run'",[]).unwrap();
    assert!(!call(&mut c, json!({"operation":"list"})).unwrap().instances[0].running);
}

#[test]
fn workflow_conversation_archive_and_restore_require_binding_review() {
    let mut c = setup();
    let created = call(&mut c, create("instance", None)).unwrap();
    let id = &created.instances[0].bindings[0].conversation_id;
    assert!(c
        .execute("UPDATE conversations SET archived_at=2 WHERE id=?1", [id])
        .unwrap_err()
        .to_string()
        .contains("workflow_active_archive_blocked"));
    call(&mut c,json!({"operation":"setInstanceEnabled","id":"instance","enabled":false,"expectedRevision":1})).unwrap();
    c.execute("UPDATE conversations SET archived_at=2 WHERE id=?1", [id])
        .unwrap();
    let archived = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert!(archived.instances[0].needs_review);
    assert_eq!(archived.instances[0].revision, 3);
    assert_eq!(archived.instances[0].bindings.len(), 1);
    let mut update = create("instance", Some(id));
    update["expectedRevision"] = json!(3);
    assert!(
        matches!(call(&mut c,update),Err(Error::Invalid(message)) if message=="workflow_conversation_archived")
    );
    c.execute(
        "UPDATE conversations SET archived_at=NULL WHERE id=?1",
        [id],
    )
    .unwrap();
    let restored = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert!(restored.instances[0].needs_review);
    assert_eq!(restored.instances[0].revision, 4);
}

#[test]
fn workflow_publish_does_not_delete_a_newer_editing_draft() {
    let mut c = setup();
    let mut first = definition();
    first["name"] = json!("First draft");
    call(&mut c,json!({"operation":"saveDraft","definition":first,"expectedRevision":2,"expectedDraftRevision":0})).unwrap();
    let mut newer = first.clone();
    newer["name"] = json!("Newer draft");
    call(&mut c,json!({"operation":"saveDraft","definition":newer,"expectedRevision":2,"expectedDraftRevision":1})).unwrap();
    for revision in [0, 1] {
        assert!(
            matches!(call(&mut c,json!({"operation":"save","definition":first,"expectedRevision":2,"expectedDraftRevision":revision})),Err(Error::Conflict(message)) if message=="workflow_draft_changed")
        );
    }
    let listed = call(&mut c, json!({"operation":"list"})).unwrap();
    assert_eq!(listed.records[0].definition.name, "Template");
    assert_eq!(listed.drafts[0].definition.name, "Newer draft");
    let published=call(&mut c,json!({"operation":"save","definition":newer,"expectedRevision":2,"expectedDraftRevision":2})).unwrap();
    assert_eq!(published.records[0].definition.name, "Newer draft");
    assert!(published.drafts.is_empty());
}

#[test]
fn workflow_binding_busy_target_changes_next_input_preferences_without_mutating_active_turn() {
    let mut c = setup();
    existing(&c, "busy", "P");
    let queued=json!([{"id":"queued","modelId":"queued-model","permissionMode":"custom","content":"Already queued"}]).to_string();
    c.execute("INSERT INTO composer_drafts(scope_id,message,permission_mode,permission_mode_version,model_id,project_id,attachments_json,skills_json,queued_messages_json,updated_at) VALUES ('busy','Still typing','default',2,'selected-model','P','[]','[]',?1,9)",[&queued]).unwrap();
    let active_run = json!({"modelId":"existing-model","permissionMode":"default"}).to_string();
    c.execute("INSERT INTO messages(id,conversation_id,role,content,agent_run_json,created_at,position) VALUES ('busy-answer','busy','assistant','Partial reply',?1,1,0)",[&active_run]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES ('busy-answer','busy','busy-run',1,'in_progress',0,1,1)",[]).unwrap();
    let bound = call(&mut c, create("instance", Some("busy"))).unwrap();
    assert_eq!(bound.affected_conversation_ids, vec!["busy"]);
    let prefs:(String,String,String,String)=c.query_row("SELECT model_id,permission_mode,message,queued_messages_json FROM composer_drafts WHERE scope_id='busy'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(
        prefs,
        (
            "model-default".into(),
            "full".into(),
            "Still typing".into(),
            queued
        )
    );
    let metadata:(String,String,String)=c.query_row("SELECT c.model_id,m.content,m.agent_run_json FROM conversations c JOIN messages m ON m.conversation_id=c.id WHERE c.id='busy'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        metadata,
        ("existing-model".into(), "Partial reply".into(), active_run)
    );
    let status: String = c
        .query_row(
            "SELECT terminal_status FROM conversation_turn_traces WHERE run_id='busy-run'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "in_progress");
    assert!(bound.instances[0].running);
    assert!(
        matches!(call(&mut c,json!({"operation":"save","definition":definition(),"expectedRevision":2,"expectedUsageRevision":bound.usages[0].usage_revision})),Err(Error::Conflict(message)) if message=="workflow_template_running")
    );
}

#[test]
fn workflow_legacy_boundary_record_is_isolated_without_conversion_and_can_be_deleted() {
    let mut c = setup();
    let mut old = definition();
    old["id"] = json!("legacy");
    old["name"] = json!("Legacy");
    old["boundaryPositions"]["output"] = json!({"x":760,"y":220});
    let raw = old.to_string();
    c.execute("INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at,enabled) VALUES ('legacy',?1,7,9,0)",[&raw]).unwrap();
    for operation in ["list", "listInstances"] {
        let listed = call(&mut c, json!({"operation":operation})).unwrap();
        assert_eq!(listed.records.len(), 1);
        assert_eq!(listed.invalid_records.len(), 1);
        let recovery = &listed.invalid_records[0];
        assert_eq!(recovery.id, "legacy");
        assert_eq!(recovery.name, "Legacy");
        assert_eq!(recovery.revision, 7);
        assert_eq!(recovery.reason, InvalidRecordReason::IncompatibleDefinition);
    }
    let mut fresh = definition();
    fresh["id"] = json!("fresh");
    let saved = call(
        &mut c,
        json!({"operation":"save","definition":fresh,"expectedRevision":0}),
    )
    .unwrap();
    assert_eq!(saved.records.len(), 2);
    assert_eq!(saved.invalid_records.len(), 1);
    let persisted: String = c
        .query_row(
            "SELECT definition_json FROM workflow_definitions WHERE workflow_id='legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(persisted, raw);
    assert!(call(
        &mut c,
        json!({"operation":"delete","id":"legacy","expectedRevision":6})
    )
    .is_err());
    let recovered = call(
        &mut c,
        json!({"operation":"delete","id":"legacy","expectedRevision":7}),
    )
    .unwrap();
    assert!(recovered.invalid_records.is_empty());
    assert_eq!(recovered.records.len(), 2);
}

#[test]
fn workflow_invalid_topology_is_isolated_and_recovery_metadata_does_not_expose_raw_graph() {
    let mut c = setup();
    let mut broken = definition();
    broken["id"] = json!("invalid");
    broken["name"] = json!("\nunsafe name");
    broken["flows"][0]["source"] = json!({"kind":"node","nodeId":"a"});
    broken["background"] = json!("PRIVATE_GRAPH_CONTENT");
    let raw = broken.to_string();
    c.execute("INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at,enabled) VALUES ('invalid',?1,1,1,0)",[&raw]).unwrap();
    let listed = call(&mut c, json!({"operation":"list"})).unwrap();
    assert_eq!(listed.records.len(), 1);
    assert_eq!(
        listed.invalid_records[0].reason,
        InvalidRecordReason::InvalidDefinition
    );
    assert_eq!(listed.invalid_records[0].name, "invalid");
    assert!(!serde_json::to_string(&listed)
        .unwrap()
        .contains("PRIVATE_GRAPH_CONTENT"));
    let persisted: String = c
        .query_row(
            "SELECT definition_json FROM workflow_definitions WHERE workflow_id='invalid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(persisted, raw);
}

#[test]
fn workflow_invalid_drafts_are_isolated_from_published_templates_and_deleted_explicitly() {
    for incompatible in [true, false] {
        let mut c = setup();
        let mut draft = definition();
        draft["name"] = json!("Unpublished changes");
        draft["background"] = json!("PRIVATE_DRAFT_CONTENT");
        let expected_reason = if incompatible {
            draft["boundaryPositions"]["output"] = json!({"x":760,"y":220});
            InvalidRecordReason::IncompatibleDefinition
        } else {
            draft["flows"][0]["source"] = json!({"kind":"node","nodeId":"a"});
            InvalidRecordReason::InvalidDefinition
        };
        let raw = draft.to_string();
        c.execute("INSERT INTO workflow_editing_drafts(template_id,definition_json,base_revision,revision,updated_at) VALUES ('template',?1,2,3,9)",[&raw]).unwrap();
        for operation in ["list", "listInstances"] {
            let listed = call(&mut c, json!({"operation":operation})).unwrap();
            assert_eq!(listed.records.len(), 1);
            assert_eq!(listed.records[0].definition.name, "Template");
            assert!(listed.drafts.is_empty());
            assert!(listed.invalid_records.is_empty());
            let recovery = &listed.invalid_drafts[0];
            assert_eq!(recovery.id, "template");
            assert_eq!(recovery.name, "Unpublished changes");
            assert_eq!(recovery.base_revision, 2);
            assert_eq!(recovery.revision, 3);
            assert_eq!(recovery.reason, expected_reason);
            assert!(!serde_json::to_string(&listed)
                .unwrap()
                .contains("PRIVATE_DRAFT_CONTENT"));
        }
        let mut fresh = definition();
        fresh["id"] = json!("fresh");
        let saved = call(
            &mut c,
            json!({"operation":"save","definition":fresh,"expectedRevision":0}),
        )
        .unwrap();
        assert_eq!(saved.records.len(), 2);
        assert_eq!(saved.invalid_drafts.len(), 1);
        let persisted: String = c
            .query_row(
                "SELECT definition_json FROM workflow_editing_drafts WHERE template_id='template'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persisted, raw);
        assert!(call(
            &mut c,
            json!({"operation":"deleteDraft","id":"template","expectedDraftRevision":2})
        )
        .is_err());
        let recovered = call(
            &mut c,
            json!({"operation":"deleteDraft","id":"template","expectedDraftRevision":3}),
        )
        .unwrap();
        assert!(recovered.invalid_drafts.is_empty());
        assert_eq!(recovered.records.len(), 2);
        assert_eq!(
            recovered
                .records
                .iter()
                .find(|r| r.definition.id == "template")
                .unwrap()
                .revision,
            2
        );
    }
}

#[test]
fn workflow_incompatible_template_remains_protected_while_used_by_an_instance() {
    let mut c = setup();
    call(&mut c, create("instance", None)).unwrap();
    let mut old = definition();
    old["boundaryPositions"]["output"] = json!({"x":760,"y":220});
    let raw = old.to_string();
    c.execute(
        "UPDATE workflow_definitions SET definition_json=?1 WHERE workflow_id='template'",
        [&raw],
    )
    .unwrap();
    let listed = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert_eq!(listed.invalid_records.len(), 1);
    assert_eq!(listed.instances.len(), 1);
    assert!(
        matches!(call(&mut c,json!({"operation":"delete","id":"template","expectedRevision":2})),Err(Error::Conflict(message)) if message=="workflow_template_in_use")
    );
    let persisted: String = c
        .query_row(
            "SELECT definition_json FROM workflow_definitions WHERE workflow_id='template'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(persisted, raw);
}

#[test]
fn workflow_default_project_only_applies_to_new_conversations_and_can_be_cleared() {
    let mut c = setup();
    existing(&c, "elsewhere", "other");
    c.execute("INSERT INTO projects(id,name,created_at,updated_at) VALUES ('destination','Destination',1,1),('next','Next',1,1)", []).unwrap();
    let mut request = create("instance", None);
    request["projectId"] = json!("destination");
    let created = call(&mut c, request.clone()).unwrap();
    let instance = &created.instances[0];
    let chat = &instance.bindings[0].conversation_id;
    assert_eq!(instance.project_id.as_deref(), Some("destination"));
    let project = |c: &Connection, chat: &str| -> Option<String> {
        c.query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [chat],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(project(&c, chat).as_deref(), Some("destination"));
    assert_eq!(
        call(&mut c, request).unwrap().instances[0].bindings,
        instance.bindings
    );
    assert_eq!(count(&c, "conversations"), 2);

    // Changing the destination must not relocate a conversation already bound to the workflow.
    let mut update = create("instance", Some(chat));
    update["projectId"] = json!("next");
    update["expectedRevision"] = json!(1);
    let changed = call(&mut c, update).unwrap();
    assert_eq!(changed.instances[0].project_id.as_deref(), Some("next"));
    assert!(changed.affected_conversation_ids.is_empty());
    assert_eq!(project(&c, chat).as_deref(), Some("destination"));

    // A manually selected conversation remains in its original project.
    let mut update = create("instance", Some("elsewhere"));
    update["projectId"] = json!("destination");
    update["expectedRevision"] = json!(2);
    call(&mut c, update).unwrap();
    assert_eq!(project(&c, "elsewhere").as_deref(), Some("other"));

    // Explicitly clearing the destination creates an unassigned conversation.
    let mut update = create("instance", None);
    update["projectId"] = Value::Null;
    update["expectedRevision"] = json!(3);
    let cleared = call(&mut c, update).unwrap();
    assert_eq!(cleared.instances[0].project_id, None);
    assert_eq!(
        project(&c, &cleared.instances[0].bindings[0].conversation_id),
        None
    );
}

#[test]
fn workflow_missing_default_project_rejects_atomically_and_deleted_project_clears_preference() {
    let mut c = setup();
    let mut request = create("instance", None);
    request["projectId"] = json!("gone");
    assert!(
        matches!(call(&mut c, request.clone()), Err(Error::Invalid(message)) if message == "workflow_project_missing")
    );
    assert_eq!(count(&c, "conversations"), 0);
    assert_eq!(count(&c, "workflow_instances"), 0);
    c.execute(
        "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('gone','Gone',1,1)",
        [],
    )
    .unwrap();
    existing(&c, "survivor", "other");
    request["bindings"] = json!([{ "nodeId": "a", "conversationId": "survivor" }]);
    let created = call(&mut c, request).unwrap();
    crate::storage::project_repository::delete_project(&c, "gone").unwrap();
    let listed = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert_eq!(listed.instances[0].project_id, None);
    assert_eq!(listed.instances[0].bindings, created.instances[0].bindings);
    assert!(listed.instances[0].enabled);
    assert_eq!(
        c.query_row(
            "SELECT project_id FROM conversations WHERE id=?1",
            [&created.instances[0].bindings[0].conversation_id],
            |r| r.get::<_, Option<String>>(0)
        )
        .unwrap(),
        Some("other".to_owned())
    );
}
