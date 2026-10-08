use super::*;
use crate::organization_personnel::TrustedEditContext;
use crate::storage::{
    composer_draft_repository, migrations::run_migrations, preferences_repository,
};
use crate::workflow::{Definition, ManagementRole, NodeConfig, WorkflowPermissionMode};
use crate::workflow_execution::{SendOutput, SendRequest};
use serde_json::{json, Value};
use std::collections::HashSet;

fn manage(c: &mut Connection, request: &Request) -> Result<Receipt, String> {
    super::manage(
        c,
        request,
        &HashSet::from(["model".into(), "current-model".into()]),
    )
}

fn instance(c: &Connection) -> crate::workflow_management::Instance {
    workflow_repository::load_instance(c, "instance")
        .unwrap()
        .unwrap()
}
fn conversation(c: &Connection, node: &str) -> String {
    c.query_row("SELECT conversation_id FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id=?1",[node],|r|r.get(0)).unwrap()
}
fn membership(c: &Connection, node: &str) -> String {
    c.query_row("SELECT membership_id FROM workflow_instance_bindings WHERE instance_id='instance' AND node_id=?1",[node],|r|r.get(0)).unwrap()
}
fn start(c: &mut Connection, node: &str, run: &str) {
    let chat = conversation(c, node);
    c.execute("INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES(?1,?2,'assistant','Prior history',1,(SELECT COALESCE(MAX(position),-1)+1 FROM messages WHERE conversation_id=?2))",params![run,chat]).unwrap();
    c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES(?1,?2,?1,?3,'in_progress',0,1,1)",params![run,chat,crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION]).unwrap();
    workflow_execution_repository::bind_run(c, &chat, run).unwrap();
}
fn fingerprint(c: &Connection) -> String {
    let p = preferences_repository::load_ui_preferences(c).unwrap();
    serde_json::to_string(&(
        p.full_permission_enabled,
        p.custom_permission_enabled,
        p.custom_permissions,
    ))
    .unwrap()
}
fn fixture() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    for id in ["model", "current-model", "disabled"] {
        c.execute("INSERT INTO models(id,provider_model_id,display_name,normalized_display_name,supports_image,provider_profile_config_json,provider_connection_revision,provider_protocol_revision,input_price,output_price,enabled,position,created_at,updated_at) VALUES(?1,?1,?1,?1,0,'{}','provider-connection-v1:test','provider-protocol-v1:test','0','0',?2,0,1,1)",params![id,id!="disabled"]).unwrap();
    }
    let node = |id: &str, rank: u8, role: &str, dept: &str| json!({"kind":"agent","id":id,"name":id,"rank":rank,"managementRole":role,"departmentId":dept,"x":40,"y":70,"permissionMode":"default","modelConfigId":"model","receives":"Source","task":"Original task","delivers":"Report"});
    let department = |id: &str, parent: Option<&str>| json!({"id":id,"name":id,"parentId":parent,"x":0,"y":0,"width":900,"height":900});
    let mut definition:Definition=serde_json::from_value(json!({"schemaVersion":1,"id":"template","name":"Team","description":"","background":"Shared context","viewport":{"x":17,"y":29,"zoom":1.25},"nodes":[node("admin",90,"organization_admin","a"),node("target",20,"department_admin","child"),node("peer",90,"member","other")],"departments":[department("a",None),department("child",Some("a")),department("other",None),department("empty",None),department("leaf",Some("empty"))]})).unwrap();
    geometry::layout(&mut definition).unwrap();
    let models = HashSet::from(["model".into(), "current-model".into()]);
    workflow_repository::request(
        &mut c,
        crate::workflow::Request::Save {
            definition,
            expected_revision: 0,
        },
        &models,
    )
    .unwrap();
    workflow_repository::request(&mut c,serde_json::from_value(json!({"operation":"saveInstance","id":"instance","templateId":"template","name":"Team","color":"#123456","expectedRevision":0,"expectedTemplateRevision":1,"bindings":[]})).unwrap(),&models).unwrap();
    start(&mut c, "admin", "admin-run");
    start(&mut c, "target", "target-run");
    c
}
fn request(c: &Connection, call: &str, changes: Value) -> Request {
    let chat = conversation(c, "admin");
    let snapshot = workflow_execution_repository::snapshot_for_run(c, &chat, "admin-run")
        .unwrap()
        .unwrap();
    Request {
        model_input: None,
        conversation_id: chat,
        source_run_id: "admin-run".into(),
        tool_call_id: call.into(),
        execution_version: snapshot.execution_version,
        expected_revision: instance(c).revision,
        input: serde_json::from_value(json!({"reason":"Arrange team","changes":changes})).unwrap(),
        context: TrustedEditContext {
            current_model_id: "current-model".into(),
            permission_settings_fingerprint: fingerprint(c),
            default_permission_mode: Some(WorkflowPermissionMode::Default),
            allowed_permission_modes: vec![
                WorkflowPermissionMode::Default,
                WorkflowPermissionMode::Custom,
                WorkflowPermissionMode::Full,
            ],
        },
    }
}
fn edit(c: &mut Connection, changes: Value) -> Result<Receipt, String> {
    let call = format!("call-{}", instance(c).revision);
    let req = request(c, &call, changes);
    manage(c, &req)
}
fn save(c: &Connection, change: impl FnOnce(&mut Definition)) {
    let mut i = instance(c);
    change(&mut i.definition);
    workflow_repository::save_instance_definition(c, &i.id, &i.definition, i.revision).unwrap();
}
fn mail(c: &mut Connection) -> crate::workflow_execution::SendReceipt {
    let chat = conversation(c, "admin");
    let snapshot = workflow_execution_repository::snapshot_for_run(c, &chat, "admin-run")
        .unwrap()
        .unwrap();
    workflow_execution_repository::send(
        c,
        &SendRequest {
            model_input: None,
            recipient_versions: Default::default(),
            conversation_id: chat,
            source_run_id: "admin-run".into(),
            tool_call_id: "mail".into(),
            execution_version: snapshot.execution_version,
            messages: vec![SendOutput {
                target_node_id: "target".into(),
                message: "Existing delivery".into(),
                reply_to_message_id: None,
            }],
        },
    )
    .unwrap()
}
fn snapshot(c: &Connection) -> Value {
    json!({"instance":instance(c),"conversations":c.query_row("SELECT count(*) FROM conversations",[],|r|r.get::<_,i64>(0)).unwrap(),"events":c.query_row("SELECT count(*) FROM workflow_mail_events",[],|r|r.get::<_,i64>(0)).unwrap(),"receipts":c.query_row("SELECT count(*) FROM organization_personnel_receipts",[],|r|r.get::<_,i64>(0)).unwrap()})
}
fn event_count(c: &Connection, kind: &str) -> i64 {
    c.query_row(
        "SELECT count(*) FROM workflow_mail_events WHERE kind=?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn organization_edit_task_preserves_admin_identity_history_mail_and_geometry() {
    let mut c = fixture();
    let chat = conversation(&c, "target");
    let membership = membership(&c, "target");
    let before = instance(&c);
    let sent = mail(&mut c);
    let run_before = workflow_execution_repository::snapshot_for_run(&c, &chat, "target-run")
        .unwrap()
        .unwrap();
    let draft_before = composer_draft_repository::get_composer_draft(&c, &chat)
        .unwrap()
        .unwrap();
    let receipt=edit(&mut c,json!([{"action":"update_member","memberId":"target","task":"Inspect every incoming document"}])).unwrap();
    let after = instance(&c);
    let node = after
        .definition
        .nodes
        .iter()
        .find(|n| n.id == "target")
        .unwrap();
    let old = before
        .definition
        .nodes
        .iter()
        .find(|n| n.id == "target")
        .unwrap();
    assert_eq!(node.management_role, ManagementRole::DepartmentAdmin);
    assert_eq!(node.department_id, old.department_id);
    assert_eq!(node.rank, old.rank);
    assert_eq!((node.x, node.y), (old.x, old.y));
    assert_eq!(
        serde_json::to_value(after.definition.viewport).unwrap(),
        serde_json::to_value(before.definition.viewport).unwrap()
    );
    assert_eq!(conversation(&c, "target"), chat);
    assert_eq!(self::membership(&c, "target"), membership);
    assert_eq!(
        serde_json::to_value(after.bindings).unwrap(),
        serde_json::to_value(before.bindings).unwrap()
    );
    let run_after = workflow_execution_repository::snapshot_for_run(&c, &chat, "target-run")
        .unwrap()
        .unwrap();
    assert_eq!(run_after.execution_version, run_before.execution_version);
    assert_eq!(run_after.task, "Inspect every incoming document");
    assert_eq!(run_after.management_role, ManagementRole::DepartmentAdmin);
    assert_eq!(
        c.query_row(
            "SELECT content FROM messages WHERE id='target-run'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Prior history"
    );
    assert_eq!(
        c.query_row(
            "SELECT mail_status FROM workflow_mail_messages WHERE input_id=?1",
            [&sent.input_ids[0]],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "pending"
    );
    assert_eq!(c.query_row("SELECT json_extract(message_json,'$.id') FROM workflow_mail_messages WHERE input_id=?1",[&sent.input_ids[0]],|r|r.get::<_,String>(0)).unwrap(),sent.messages[0].id);
    assert_eq!(
        serde_json::to_value(
            composer_draft_repository::get_composer_draft(&c, &chat)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(draft_before).unwrap()
    );
    assert_eq!(receipt.changes[0].fields.len(), 1);
    assert_eq!(receipt.changes[0].fields[0].field, "task");
    assert_eq!(receipt.changes[0].conversation_id, Some(chat));
    assert_eq!(event_count(&c, "members_changed"), 1);
    assert_eq!(event_count(&c, "member_permissions_changed"), 0);
}
#[test]
fn organization_edit_add_defaults_actual_run_model_permissions_and_explicit_root() {
    let mut c = fixture();
    let receipt=edit(&mut c,json!([{"action":"add_member","name":"Junior","task":"Help"},{"action":"add_member","name":"Root","task":"Help","departmentId":null,"managementRole":"organization_admin","rank":30}])).unwrap();
    let i = instance(&c);
    let added = i
        .definition
        .nodes
        .iter()
        .find(|n| n.name == "Junior")
        .unwrap();
    assert_eq!(added.rank, 1);
    assert_eq!(added.management_role, ManagementRole::Member);
    assert_eq!(added.department_id.as_deref(), Some("a"));
    let NodeConfig::Agent(config) = &added.config;
    assert_eq!(config.model_config_id.as_deref(), Some("current-model"));
    assert_eq!(config.permission_mode, WorkflowPermissionMode::Default);
    assert_eq!(
        i.definition
            .nodes
            .iter()
            .find(|n| n.name == "Root")
            .unwrap()
            .department_id,
        None
    );
    assert_eq!(receipt.affected_conversation_ids.len(), 2);
    assert_eq!(receipt.changes[0].entity_type, "member");
    let label = receipt.changes[0]
        .fields
        .iter()
        .find(|f| f.field == "modelConfigId")
        .unwrap();
    assert_eq!(label.after_label.as_deref(), Some("current-model"));
    assert_eq!(i.definition.viewport.x, 17.);
}
#[test]
fn organization_edit_clear_text_and_root_preserve_explicit_roles() {
    let mut c = fixture();
    let before = snapshot(&c);
    assert!(edit(
        &mut c,
        json!([{"action":"update_member","memberId":"target","departmentId":null}])
    )
    .unwrap_err()
    .contains("must belong to a department"));
    assert_eq!(snapshot(&c), before);
    let result=edit(&mut c,json!([{"action":"update_member","memberId":"target","receives":"","delivers":"","departmentId":null,"managementRole":"organization_admin","rank":80}])).unwrap();
    let i = instance(&c);
    let n = i
        .definition
        .nodes
        .iter()
        .find(|n| n.id == "target")
        .unwrap();
    assert_eq!(n.management_role, ManagementRole::OrganizationAdmin);
    assert_eq!(n.department_id, None);
    let NodeConfig::Agent(a) = &n.config;
    assert_eq!(a.receives, "");
    assert_eq!(a.delivers, "");
    assert_eq!(a.task, "Original task");
    let field = result.changes[0]
        .fields
        .iter()
        .find(|f| f.field == "departmentId")
        .unwrap();
    assert_eq!(field.before_label.as_deref(), Some("a/child"));
    assert!(field.after.is_null());
}
#[test]
fn organization_edit_authority_checks_old_new_rank_roles_and_department_scope() {
    let mut c = fixture();
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::DepartmentAdmin
    });
    for change in [
        json!({"action":"update_member","memberId":"admin","rank":1}),
        json!({"action":"update_member","memberId":"peer","rank":1}),
        json!({"action":"update_member","memberId":"target","rank":90}),
        json!({"action":"update_member","memberId":"target","departmentId":"other"}),
        json!({"action":"update_member","memberId":"target","managementRole":"organization_admin"}),
        json!({"action":"add_member","name":"Root","task":"Work","departmentId":null}),
        json!({"action":"update_department","departmentId":"other","name":"Outside"}),
    ] {
        let before = snapshot(&c);
        assert!(edit(&mut c, json!([change])).is_err());
        assert_eq!(snapshot(&c), before);
    }
    edit(&mut c,json!([{"action":"update_member","memberId":"target","rank":40,"managementRole":"department_admin","name":"Lead"}])).unwrap();
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::Member
    });
    assert!(edit(
        &mut c,
        json!([{"action":"remove_member","memberId":"target"}])
    )
    .unwrap_err()
    .contains("denied"));
}
#[test]
fn organization_edit_permissions_bound_result_and_actual_next_composer_without_downgrade() {
    let mut c = fixture();
    let chat = conversation(&c, "target");
    composer_draft_repository::set_composer_configuration(&c, &chat, Some("model"), "full", 1)
        .unwrap();
    let mut req = request(
        &c,
        "text",
        json!([{"action":"update_member","memberId":"target","task":"Updated"}]),
    );
    req.context.allowed_permission_modes = vec![WorkflowPermissionMode::Default];
    let before = snapshot(&c);
    assert!(manage(&mut c, &req)
        .unwrap_err()
        .contains("organization_permission_ceiling"));
    assert_eq!(snapshot(&c), before);
    assert_eq!(
        composer_draft_repository::get_composer_draft(&c, &chat)
            .unwrap()
            .unwrap()
            .permission_mode,
        "full"
    );
    req.input=serde_json::from_value(json!({"reason":"Adjust","changes":[{"action":"update_member","memberId":"target","task":"Updated","permissionMode":"default"}]})).unwrap();
    manage(&mut c, &req).unwrap();
    assert_eq!(
        composer_draft_repository::get_composer_draft(&c, &chat)
            .unwrap()
            .unwrap()
            .permission_mode,
        "default"
    );
    save(&c, |d| {
        let NodeConfig::Agent(a) = &mut d
            .nodes
            .iter_mut()
            .find(|n| n.id == "target")
            .unwrap()
            .config;
        a.permission_mode = WorkflowPermissionMode::Full;
    });
    let mut req = request(
        &c,
        "retained-full",
        json!([{"action":"update_member","memberId":"target","name":"Updated"}]),
    );
    req.context.allowed_permission_modes = vec![WorkflowPermissionMode::Default];
    assert!(manage(&mut c, &req).unwrap_err().contains("ceiling"));
}
#[test]
fn organization_edit_explicit_configuration_updates_only_next_selected_fields_and_events() {
    let mut c = fixture();
    let chat = conversation(&c, "target");
    let mut draft = composer_draft_repository::get_composer_draft(&c, &chat)
        .unwrap()
        .unwrap();
    draft.model_id = Some("current-model".into());
    draft.permission_mode = "full".into();
    draft.message = "Unsent message".into();
    draft.queued_messages_json = "[{\"id\":\"queue\",\"content\":\"Queued\"}]".into();
    draft.updated_at += 100;
    composer_draft_repository::save_composer_draft(&c, draft.clone()).unwrap();
    let membership = membership(&c, "target");
    let result=edit(&mut c,json!([{"action":"update_member","memberId":"target","modelConfigId":"model","permissionMode":"default"}])).unwrap();
    let after = composer_draft_repository::get_composer_draft(&c, &chat)
        .unwrap()
        .unwrap();
    assert_eq!(after.message, draft.message);
    assert_eq!(after.queued_messages_json, draft.queued_messages_json);
    assert_eq!(after.model_id.as_deref(), Some("model"));
    assert_eq!(after.permission_mode, "default");
    assert_eq!(membership, self::membership(&c, "target"));
    assert_eq!(event_count(&c, "member_model_changed"), 1);
    assert_eq!(event_count(&c, "member_permissions_changed"), 1);
    assert_eq!(result.changes[0].fields.len(), 2);
    assert_eq!(
        result.changes[0]
            .fields
            .iter()
            .find(|f| f.field == "modelConfigId")
            .unwrap()
            .before,
        json!("current-model")
    );
    assert_eq!(
        c.query_row(
            "SELECT terminal_status FROM conversation_turn_traces WHERE run_id='target-run'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "in_progress"
    );
    edit(
        &mut c,
        json!([{"action":"update_member","memberId":"target","modelConfigId":"current-model"}]),
    )
    .unwrap();
    assert_eq!(
        composer_draft_repository::get_composer_draft(&c, &chat)
            .unwrap()
            .unwrap()
            .permission_mode,
        "default"
    );
    assert_eq!(event_count(&c, "member_permissions_changed"), 1);
}
#[test]
fn organization_edit_unrepresentable_defaults_disabled_model_and_atomic_rollback() {
    let mut c = fixture();
    let before = snapshot(&c);
    let mut req = request(
        &c,
        "permission",
        json!([{"action":"add_member","name":"New","task":"Work"}]),
    );
    req.context.default_permission_mode = None;
    assert!(manage(&mut c, &req)
        .unwrap_err()
        .contains("not_representable"));
    assert_eq!(snapshot(&c), before);
    req.input=serde_json::from_value(json!({"reason":"Explicit","changes":[{"action":"add_member","name":"New","task":"Work","permissionMode":"default"}]})).unwrap();
    manage(&mut c, &req).unwrap();
    for change in [
        json!({"action":"add_member","name":"Broken","task":"Work","modelConfigId":"disabled"}),
        json!({"action":"update_member","memberId":"target","modelConfigId":"missing"}),
    ] {
        let before = snapshot(&c);
        assert!(edit(&mut c, json!([change])).is_err());
        assert_eq!(snapshot(&c), before);
    }
    let before = snapshot(&c);
    assert!(edit(&mut c,json!([{"action":"add_member","name":"Rollback","task":"Work"},{"action":"add_department","name":"Rollback department"},{"action":"remove_member","memberId":"peer"}])).is_err());
    assert_eq!(snapshot(&c), before);
    let chat = conversation(&c, "target");
    let draft =
        serde_json::to_value(composer_draft_repository::get_composer_draft(&c, &chat).unwrap())
            .unwrap();
    assert!(edit(&mut c,json!([{"action":"update_member","memberId":"target","modelConfigId":"current-model","permissionMode":"full"},{"action":"remove_member","memberId":"peer"}])).is_err());
    assert_eq!(snapshot(&c), before);
    assert_eq!(
        serde_json::to_value(composer_draft_repository::get_composer_draft(&c, &chat).unwrap())
            .unwrap(),
        draft
    );
}
#[test]
fn organization_edit_department_rename_keeps_geometry_and_reparent_checks_indirect_authority() {
    let mut c = fixture();
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::DepartmentAdmin
    });
    let before = instance(&c);
    edit(
        &mut c,
        json!([{"action":"update_department","departmentId":"a","name":"Our department"}]),
    )
    .unwrap();
    let after = instance(&c);
    let old = before
        .definition
        .departments
        .iter()
        .find(|d| d.id == "a")
        .unwrap();
    let new = after
        .definition
        .departments
        .iter()
        .find(|d| d.id == "a")
        .unwrap();
    assert_eq!(
        (new.x, new.y, new.width, new.height),
        (old.x, old.y, old.width, old.height)
    );
    for change in [
        json!({"action":"update_department","departmentId":"a","parentId":"child"}),
        json!({"action":"update_department","departmentId":"child","parentId":"other"}),
        json!({"action":"update_department","departmentId":"other","parentId":"a"}),
    ] {
        let before = snapshot(&c);
        assert!(edit(&mut c, json!([change])).is_err());
        assert_eq!(snapshot(&c), before);
    }
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::OrganizationAdmin
    });
    for change in [
        json!({"action":"update_department","departmentId":"a","parentId":"empty"}),
        json!({"action":"update_department","departmentId":"other","parentId":"empty"}),
        json!({"action":"update_department","departmentId":"empty","parentId":"leaf"}),
    ] {
        let before = snapshot(&c);
        assert!(edit(&mut c, json!([change])).is_err());
        assert_eq!(snapshot(&c), before);
    }
    edit(
        &mut c,
        json!([{"action":"update_department","departmentId":"child","parentId":null}]),
    )
    .unwrap();
    assert_eq!(
        instance(&c)
            .definition
            .departments
            .iter()
            .find(|d| d.id == "child")
            .unwrap()
            .parent_id,
        None
    );
}
#[test]
fn organization_edit_department_defaults_empty_deletion_and_no_batch_scope_widening() {
    let mut c = fixture();
    let before = snapshot(&c);
    assert!(edit(
        &mut c,
        json!([{"action":"remove_department","departmentId":"empty"}])
    )
    .is_err());
    assert_eq!(snapshot(&c), before);
    edit(&mut c,json!([{"action":"remove_department","departmentId":"leaf"},{"action":"remove_department","departmentId":"empty"}])).unwrap();
    assert!(instance(&c)
        .definition
        .departments
        .iter()
        .all(|d| d.id != "empty" && d.id != "leaf"));
    let r = edit(
        &mut c,
        json!([{"action":"add_department","name":"Root department"}]),
    )
    .unwrap();
    assert_eq!(
        instance(&c)
            .definition
            .departments
            .iter()
            .find(|d| d.id == r.changes[0].entity_id)
            .unwrap()
            .parent_id,
        None
    );
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::DepartmentAdmin
    });
    let r = edit(
        &mut c,
        json!([{"action":"add_department","name":"Our child"}]),
    )
    .unwrap();
    assert_eq!(
        instance(&c)
            .definition
            .departments
            .iter()
            .find(|d| d.id == r.changes[0].entity_id)
            .unwrap()
            .parent_id
            .as_deref(),
        Some("a")
    );
    let before = snapshot(&c);
    assert!(edit(&mut c,json!([{"action":"update_department","departmentId":"other","parentId":"a"},{"action":"update_member","memberId":"peer","rank":10}])).is_err());
    assert_eq!(snapshot(&c), before);
}
#[test]
fn organization_edit_remove_preserves_conversation_invalidates_only_removed_membership() {
    let mut c = fixture();
    let chat = conversation(&c, "target");
    let admin = conversation(&c, "admin");
    let identity = workflow_execution_repository::snapshot_for_run(&c, &admin, "admin-run")
        .unwrap()
        .unwrap()
        .execution_version;
    let sent = mail(&mut c);
    edit(
        &mut c,
        json!([{"action":"remove_member","memberId":"target"}]),
    )
    .unwrap();
    assert!(
        workflow_execution_repository::snapshot_for_run(&c, &chat, "target-run")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        workflow_execution_repository::snapshot_for_run(&c, &admin, "admin-run")
            .unwrap()
            .unwrap()
            .execution_version,
        identity
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM messages WHERE conversation_id=?1",
            [&chat],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM conversations WHERE id=?1",
            [&chat],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT mail_status FROM workflow_mail_messages WHERE input_id=?1",
            [&sent.input_ids[0]],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "failed"
    );
}
#[test]
fn organization_edit_committed_receipt_replays_after_role_removal_settings_and_membership_change() {
    let mut c = fixture();
    let req = request(
        &c,
        "replay",
        json!([{"action":"add_member","name":"New","task":"Work"}]),
    );
    let result = manage(&mut c, &req).unwrap();
    save(&c, |d| {
        d.nodes
            .iter_mut()
            .find(|n| n.id == "admin")
            .unwrap()
            .management_role = ManagementRole::Member
    });
    c.execute(
        "DELETE FROM workflow_instance_bindings WHERE node_id='admin'",
        [],
    )
    .unwrap();
    let mut p = preferences_repository::load_ui_preferences(&c).unwrap();
    p.full_permission_enabled = !p.full_permission_enabled;
    preferences_repository::save_ui_preferences(&c, p).unwrap();
    let before = snapshot(&c);
    assert_eq!(
        serde_json::to_value(manage(&mut c, &req).unwrap()).unwrap(),
        serde_json::to_value(result).unwrap()
    );
    assert_eq!(snapshot(&c), before);
    let mut different = req.clone();
    different.input.reason = "Another reason".into();
    assert!(manage(&mut c, &different)
        .unwrap_err()
        .contains("different arguments"));
    assert!(receipt_for_call(&c, "wrong", "admin-run", "replay", &req.input).is_err());
}

#[test]
fn organization_edit_semantic_receipt_replays_without_resolving_renamed_members() {
    let mut c = fixture();
    let mut req = request(
        &c,
        "semantic-edit",
        json!([{"action":"update_member","memberId":"target","name":"Editor"}]),
    );
    let raw = json!({"reason":"Rename reviewer","changes":[{"action":"update_member","member":"target","name":"Editor"}]});
    req.model_input = Some(raw.clone());
    let receipt = manage(&mut c, &req).unwrap();
    let replay = receipt_for_model_call(
        &c,
        &req.conversation_id,
        &req.source_run_id,
        &req.tool_call_id,
        &raw,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(&receipt).unwrap()
    );
    // A repeated dispatch recovers its committed outcome even if another directory resolution
    // would now point at a different member. It cannot execute that newly resolved operation.
    let before = snapshot(&c);
    req.input = serde_json::from_value(json!({"reason":"Rename reviewer","changes":[{"action":"update_member","memberId":"peer","name":"Editor"}]})).unwrap();
    assert_eq!(
        serde_json::to_value(manage(&mut c, &req).unwrap()).unwrap(),
        serde_json::to_value(receipt).unwrap()
    );
    assert_eq!(snapshot(&c), before);
    let altered = json!({"reason":"Rename reviewer","changes":[{"action":"update_member","member":"peer","name":"Editor"}]});
    assert!(receipt_for_model_call(
        &c,
        &req.conversation_id,
        &req.source_run_id,
        &req.tool_call_id,
        &altered
    )
    .unwrap_err()
    .contains("different arguments"));
}
#[test]
fn organization_edit_rejects_stale_revision_identity_run_owner_stop_and_permission_settings() {
    let mut c = fixture();
    let req = request(
        &c,
        "stale",
        json!([{"action":"update_member","memberId":"target","task":"Work"}]),
    );
    save(&c, |d| d.background = "Changed".into());
    assert!(manage(&mut c, &req)
        .unwrap_err()
        .contains("revision_conflict"));
    let mut req = request(
        &c,
        "identity",
        json!([{"action":"update_member","memberId":"target","task":"Work"}]),
    );
    req.execution_version = "obsolete".into();
    assert!(manage(&mut c, &req).is_err());
    let mut req = request(
        &c,
        "wrong-run",
        json!([{"action":"update_member","memberId":"target","task":"Work"}]),
    );
    req.source_run_id = "target-run".into();
    assert!(manage(&mut c, &req).is_err());
    let req = request(
        &c,
        "settings",
        json!([{"action":"add_member","name":"No effect","task":"Work"}]),
    );
    let mut p = preferences_repository::load_ui_preferences(&c).unwrap();
    p.custom_permission_enabled = !p.custom_permission_enabled;
    preferences_repository::save_ui_preferences(&c, p).unwrap();
    let before = snapshot(&c);
    assert!(manage(&mut c, &req)
        .unwrap_err()
        .contains("permission_settings_changed"));
    assert_eq!(snapshot(&c), before);
    let req = request(
        &c,
        "stop",
        json!([{"action":"remove_member","memberId":"target"}]),
    );
    c.execute(
        "INSERT INTO workflow_mail_pauses(conversation_id,created_at) VALUES(?1,1)",
        [&req.conversation_id],
    )
    .unwrap();
    assert!(manage(&mut c, &req).unwrap_err().contains("unstopped"));
    c.execute("DELETE FROM workflow_mail_pauses", []).unwrap();
    c.execute("UPDATE conversation_turn_traces SET terminal_status='cancelled',completed_at=2 WHERE run_id='admin-run'",[]).unwrap();
    assert!(manage(&mut c, &req).unwrap_err().contains("unstopped"));
    c.execute("UPDATE conversation_turn_traces SET terminal_status='in_progress',completed_at=NULL WHERE run_id='admin-run'",[]).unwrap();
    c.execute("UPDATE workflow_instance_bindings SET membership_id='new-incarnation' WHERE node_id='admin'",[]).unwrap();
    assert!(manage(&mut c, &req).unwrap_err().contains("denied"));
}
#[test]
fn organization_edit_structural_layout_contains_nested_departments_without_sibling_overlap() {
    let mut c = fixture();
    edit(&mut c,json!([{"action":"add_member","name":"One","task":"Work","departmentId":"child"},{"action":"add_member","name":"Two","task":"Work","departmentId":"child"},{"action":"add_department","name":"Sibling","parentId":"a"}])).unwrap();
    let d = instance(&c).definition;
    for child in &d.departments {
        if let Some(parent) = child
            .parent_id
            .as_ref()
            .map(|id| d.departments.iter().find(|p| &p.id == id).unwrap())
        {
            assert!(
                child.x >= parent.x
                    && child.y >= parent.y + 44.
                    && child.x + child.width <= parent.x + parent.width
                    && child.y + child.height <= parent.y + parent.height
            );
        }
    }
    for n in &d.nodes {
        if let Some(p) = n
            .department_id
            .as_ref()
            .map(|id| d.departments.iter().find(|p| &p.id == id).unwrap())
        {
            assert!(
                n.x >= p.x
                    && n.y >= p.y + 44.
                    && n.x + 212. <= p.x + p.width
                    && n.y + 52. <= p.y + p.height
            );
        }
    }
    let mut rectangles = Vec::new();
    for n in &d.nodes {
        rectangles.push((n.department_id.as_deref(), n.x, n.y, 212., 52.));
    }
    for p in &d.departments {
        rectangles.push((p.parent_id.as_deref(), p.x, p.y, p.width, p.height));
    }
    for (i, a) in rectangles.iter().enumerate() {
        for b in &rectangles[i + 1..] {
            if a.0 == b.0 {
                assert!(
                    a.1 + a.3 <= b.1 || b.1 + b.3 <= a.1 || a.2 + a.4 <= b.2 || b.2 + b.4 <= a.2
                );
            }
        }
    }
    assert_eq!(d.viewport.x, 17.);
    assert_eq!(d.viewport.y, 29.);
    assert_eq!(d.viewport.zoom, 1.25);
}

#[test]
fn organization_edit_prose_does_not_visit_mail_history_and_removal_reads_only_live_mail() {
    let mut c = fixture();
    let pending = mail(&mut c);
    // A terminal history row deliberately lacks the live Input payload. Reconciliation must
    // filter terminal history in SQL, instead of materializing every historical body first.
    c.execute("INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,conversation_id,input_json,status,created_at,updated_at) VALUES('history','instance','old','target',NULL,'{}','completed',1,1)",[]).unwrap();
    c.execute("INSERT INTO workflow_mail_messages(message_id,instance_id,execution_version,node_id,mail_status,message_json,input_id,created_at) VALUES('historical-mail','instance','old','target','processed','{}','history',1)",[]).unwrap();
    let req = request(
        &c,
        "prose-without-mail-read",
        json!([{"action":"update_member","memberId":"target","task":"New task"}]),
    );
    c.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
        // SQLite authorizes trigger reads at prepare time, even though this prose event has
        // NULL input_id. The existing v70 trigger only resolves delivered-input metadata;
        // direct mail reads and all history/body access remain forbidden below.
        if context.accessor == Some("workflow_mail_change_event_insert")
            && matches!(
                context.action,
                rusqlite::hooks::AuthAction::Read {
                    table_name: "workflow_mail_inputs",
                    column_name: "input_id" | "instance_id" | "conversation_id" | "delivery_id",
                }
            )
        {
            return rusqlite::hooks::Authorization::Allow;
        }
        if matches!(
            context.action,
            rusqlite::hooks::AuthAction::Read {
                table_name: "workflow_mail_inputs" | "workflow_mail_messages",
                ..
            }
        ) {
            rusqlite::hooks::Authorization::Deny
        } else {
            rusqlite::hooks::Authorization::Allow
        }
    }));
    manage(&mut c, &req).unwrap();
    c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    edit(
        &mut c,
        json!([{"action":"remove_member","memberId":"target"}]),
    )
    .unwrap();
    assert_eq!(
        c.query_row(
            "SELECT mail_status FROM workflow_mail_messages WHERE input_id=?1",
            [&pending.input_ids[0]],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "failed"
    );
    assert_eq!(
        c.query_row(
            "SELECT input_json FROM workflow_mail_inputs WHERE input_id='history'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "{}"
    );
}

#[test]
fn organization_edit_root_insertion_and_removal_preserve_existing_layout() {
    let mut c = fixture();
    let before = instance(&c).definition;
    let receipt = edit(
        &mut c,
        json!([{"action":"add_member","name":"Root helper","task":"Help","departmentId":null}]),
    )
    .unwrap();
    let after = instance(&c).definition;
    for node in &before.nodes {
        let same = after.nodes.iter().find(|n| n.id == node.id).unwrap();
        assert_eq!((same.x, same.y), (node.x, node.y));
    }
    for frame in &before.departments {
        let same = after.departments.iter().find(|d| d.id == frame.id).unwrap();
        assert_eq!(
            (same.x, same.y, same.width, same.height),
            (frame.x, frame.y, frame.width, frame.height)
        );
    }
    edit(
        &mut c,
        json!([{"action":"remove_member","memberId":receipt.changes[0].entity_id}]),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(instance(&c).definition).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}

#[test]
fn organization_incremental_layout_expands_only_affected_frames_and_keeps_subtree_offsets() {
    let mut c = fixture();
    save(&c, |d| {
        for department in &mut d.departments {
            if department.id == "other" {
                department.x = 5000.;
                department.y = 0.;
                department.width = 1200.;
                department.height = 1200.;
            }
            if department.id == "empty" {
                department.x = 10000.;
                department.y = 0.;
            }
        }
        let peer = d.nodes.iter_mut().find(|n| n.id == "peer").unwrap();
        peer.x = 5050.;
        peer.y = 60.;
    });
    let before = instance(&c).definition;
    let source = before.departments.iter().find(|d| d.id == "child").unwrap();
    let target = before.nodes.iter().find(|n| n.id == "target").unwrap();
    let offset = (target.x - source.x, target.y - source.y);
    let peer = before.nodes.iter().find(|n| n.id == "peer").unwrap();
    edit(
        &mut c,
        json!([{"action":"update_department","departmentId":"child","parentId":"other"}]),
    )
    .unwrap();
    let after = instance(&c).definition;
    let moved = after.departments.iter().find(|d| d.id == "child").unwrap();
    let target = after.nodes.iter().find(|n| n.id == "target").unwrap();
    assert_eq!((target.x - moved.x, target.y - moved.y), offset);
    let unchanged = after.nodes.iter().find(|n| n.id == "peer").unwrap();
    assert_eq!((unchanged.x, unchanged.y), (peer.x, peer.y));
    let parent = after.departments.iter().find(|d| d.id == "other").unwrap();
    assert!(
        moved.x >= parent.x
            && moved.y >= parent.y
            && moved.x + moved.width <= parent.x + parent.width
            && moved.y + moved.height <= parent.y + parent.height
    );
}

#[test]
fn organization_edit_duplicate_member_names_roll_back_entire_batch() {
    let mut c = fixture();
    let before = snapshot(&c);
    for changes in [
        json!([{"action":"add_member","name":"　ADMIN ","task":"Work"}]),
        json!([{"action":"add_member","name":"New","task":"Work"},{"action":"add_member","name":"new ","task":"Work"}]),
        json!([{"action":"add_member","name":"Valid","task":"Work"},{"action":"update_member","memberId":"target","name":"PEER"}]),
    ] {
        let error = edit(&mut c, changes).unwrap_err();
        assert!(
            error.contains("organization_duplicate_member_name"),
            "{error}"
        );
        assert_eq!(
            snapshot(&c),
            before,
            "No members, conversations, events or receipts may commit on duplicate names"
        );
    }
    // A rename can release a name for a new member within the same atomic edit.
    edit(
        &mut c,
        json!([
            {"action":"update_member","memberId":"target","name":"Team lead"},
            {"action":"add_member","name":"target","task":"New role"}
        ]),
    )
    .unwrap();
    assert_eq!(instance(&c).definition.nodes.len(), 4);
}
