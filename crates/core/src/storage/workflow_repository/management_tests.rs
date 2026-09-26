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
        json!({"operation":"setEnabled","id":"template","enabled":true,"expectedRevision":1}),
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
fn workflow_bindings_are_exclusive_and_fail_atomically() {
    let mut c = setup();
    existing(&c, "existing", "P");
    call(&mut c, create("first", Some("existing"))).unwrap();
    assert!(matches!(
        call(&mut c, create("second", Some("existing"))),
        Err(Error::Conflict(_))
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
    c.execute("UPDATE conversations SET archived_at=2 WHERE id=?1", [id])
        .unwrap();
    let archived = call(&mut c, json!({"operation":"listInstances"})).unwrap();
    assert!(archived.instances[0].needs_review);
    assert_eq!(archived.instances[0].revision, 2);
    assert_eq!(archived.instances[0].bindings.len(), 1);
    let mut update = create("instance", Some(id));
    update["expectedRevision"] = json!(2);
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
    assert_eq!(restored.instances[0].revision, 3);
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
fn workflow_availability_changes_advance_draft_base_without_rewriting_edits() {
    let mut c = setup();
    let mut graph = definition();
    graph["name"] = json!("Editing");
    call(&mut c,json!({"operation":"saveDraft","definition":graph,"expectedRevision":2,"expectedDraftRevision":0})).unwrap();
    let toggled = call(
        &mut c,
        json!({"operation":"setEnabled","id":"template","enabled":false,"expectedRevision":2}),
    )
    .unwrap();
    assert_eq!(toggled.drafts[0].base_revision, 3);
    assert_eq!(toggled.drafts[0].revision, 1);
    assert_eq!(toggled.drafts[0].definition.name, "Editing");
    let published=call(&mut c,json!({"operation":"save","definition":graph,"expectedRevision":3,"expectedDraftRevision":1})).unwrap();
    assert_eq!(published.records[0].definition.name, "Editing");
}

#[test]
fn workflow_disabled_templates_block_new_instances_but_allow_existing_binding_updates() {
    let mut c = setup();
    let created = call(&mut c, create("existing", None)).unwrap();
    call(
        &mut c,
        json!({"operation":"setEnabled","id":"template","enabled":false,"expectedRevision":2}),
    )
    .unwrap();
    let mut new = create("new", None);
    new["expectedTemplateRevision"] = json!(3);
    assert!(call(&mut c, new).is_err());
    let mut update = create(
        "existing",
        Some(&created.instances[0].bindings[0].conversation_id),
    );
    update["expectedRevision"] = json!(1);
    update["expectedTemplateRevision"] = json!(3);
    let updated = call(&mut c, update).unwrap();
    assert_eq!(updated.instances.len(), 1);
    assert!(updated.affected_conversation_ids.is_empty());
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
