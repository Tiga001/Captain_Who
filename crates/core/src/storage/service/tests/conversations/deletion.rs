use super::*;

#[test]
fn workflow_teardown_retires_bindings_without_disabling_other_members() {
    for delete_project in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        for id in ["workflow-root", "workflow-loose"] {
            service
                .save_conversation(conversation(
                    id,
                    Some("project-1"),
                    &format!("message-{id}"),
                ))
                .unwrap();
        }
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: "workflow-root-agent".into(),
                conversation_id: "workflow-root".into(),
                creation_request_id: "workflow-root-create".into(),
                task_name: "Workflow root".into(),
            })
            .unwrap();
        {
            let c = service.state.connection().unwrap();
            c.execute("INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at) VALUES ('template','{\"schemaVersion\":1,\"id\":\"template\"}',1,1)",[]).unwrap();
            let definition = serde_json::json!({"schemaVersion":1,"id":"instance","name":"Team","description":"","background":"","nodes":[],"viewport":{"x":0,"y":0,"zoom":1}}).to_string();
            for (id, color) in [("workflow-root", "#AABBCC"), ("workflow-loose", "#001122")] {
                c.execute("INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,needs_review,running,last_request_json,enabled,definition_json) VALUES (?1,'template',1,?1,?2,1,1,0,0,'{}',1,json_set(?3,'$.id',?1))",rusqlite::params![id,color,definition]).unwrap();
                c.execute("INSERT INTO workflow_instance_bindings(instance_id,node_id,conversation_id) VALUES (?1,'node',?1)",[id]).unwrap();
            }
        }
        if delete_project {
            service.delete_project("project-1").unwrap();
        } else {
            service.delete_conversation("workflow-root").unwrap();
        }
        let c = service.state.connection().unwrap();
        let root: (bool,bool,i64)=c.query_row("SELECT enabled,needs_review,revision FROM workflow_instances WHERE instance_id='workflow-root'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(root, (true, false, 2));
        let loose: (bool,bool,i64)=c.query_row("SELECT enabled,needs_review,revision FROM workflow_instances WHERE instance_id='workflow-loose'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(
            loose,
            if delete_project {
                (true, false, 2)
            } else {
                (true, false, 1)
            }
        );
    }
}

#[test]
fn deleting_conversation_and_project_removes_composer_drafts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-2",
            Some("project-1"),
            "message-2",
        ))
        .unwrap();
    service
        .save_conversation(conversation(
            "conversation-3",
            Some("project-2"),
            "message-3",
        ))
        .unwrap();

    service
        .save_composer_draft(composer_draft(
            "conversation-1",
            Some("project-1"),
            "draft 1",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-2",
            Some("project-1"),
            "draft 2",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "new-conversation-project-1",
            Some("project-1"),
            "new draft",
        ))
        .unwrap();
    service
        .save_composer_draft(composer_draft(
            "conversation-3",
            Some("project-2"),
            "draft 3",
        ))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(
        scopes,
        vec![
            "conversation-2".to_string(),
            "conversation-3".to_string(),
            "new-conversation-project-1".to_string()
        ]
    );

    service.delete_project("project-1").unwrap();

    let mut scopes = service
        .load_composer_drafts()
        .unwrap()
        .into_iter()
        .map(|draft| draft.scope_id)
        .collect::<Vec<_>>();
    scopes.sort();
    assert_eq!(scopes, vec!["conversation-3".to_string()]);
}

#[test]
fn graph_bound_root_conversation_and_project_deletes_remove_owned_agent_trees() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    for (project_id, suffix) in [("project-1", "conversation"), ("project-2", "project")] {
        let conversation_id = format!("conversation-graph-{suffix}");
        let message_id = format!("message-graph-{suffix}");
        let agent_id = format!("agent-graph-{suffix}");
        service
            .save_conversation(conversation(
                &conversation_id,
                Some(project_id),
                &message_id,
            ))
            .unwrap();
        service
            .save_composer_draft(composer_draft(
                &conversation_id,
                Some(project_id),
                "owned draft",
            ))
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id,
                conversation_id,
                creation_request_id: format!("ensure-agent-graph-{suffix}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
    }

    {
        let connection = service.state.connection().unwrap();
        assert_eq!(
            history_timeline_projection_count(&connection, "conversation-graph-conversation"),
            1
        );
        assert_eq!(
            history_timeline_projection_count(&connection, "conversation-graph-project"),
            1
        );
    }

    service
        .delete_conversation("conversation-graph-conversation")
        .unwrap();
    assert!(service
        .load_conversation("conversation-graph-conversation")
        .unwrap()
        .is_none());
    assert!(service
        .get_agent_node("agent-graph-conversation")
        .unwrap()
        .is_none());
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));

    service.delete_project("project-2").unwrap();
    assert!(service
        .load_conversation("conversation-graph-project")
        .unwrap()
        .is_none());
    assert!(service
        .get_agent_node("agent-graph-project")
        .unwrap()
        .is_none());
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .all(|project| project.id != "project-2"));
    assert!(service
        .load_composer_drafts()
        .unwrap()
        .iter()
        .all(|draft| !draft.scope_id.starts_with("conversation-graph-")));

    let connection = service.state.connection().unwrap();
    assert_eq!(
        history_timeline_projection_count(&connection, "conversation-graph-conversation"),
        0
    );
    assert_eq!(
        history_timeline_projection_count(&connection, "conversation-graph-project"),
        0
    );
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn graph_bound_deletes_preserve_and_block_referencing_automations() {
    use crate::storage::automation_repository::{
        AutomationConfigRecord, AutomationCreateOutcome, NewAutomationRecord,
        StoredAutomationStatus,
    };

    let fixture = StorageFixture::new();
    let service = fixture.service();
    let create_referencing_automation =
        |automation_id: &str, request_id: &str, conversation_id: &str, project_id: &str| {
            match service
                .create_automation(&NewAutomationRecord {
                    id: automation_id.to_string(),
                    create_request_id: request_id.to_string(),
                    status: StoredAutomationStatus::Active,
                    config: AutomationConfigRecord {
                        title: format!("Follow {conversation_id}"),
                        prompt: "Report important changes.".to_string(),
                        health_state: "ok".to_string(),
                        blocked_code: None,
                        blocked_message: None,
                        destination_kind: "existing_chat".to_string(),
                        target_conversation_id: Some(conversation_id.to_string()),
                        project_binding_kind: "inherit".to_string(),
                        project_id: None,
                        model_id: None,
                        permission_mode: "default".to_string(),
                        permission_mode_version: 2,
                        permissions_json: "{}".to_string(),
                        reasoning_json: None,
                        schedule_kind: "daily".to_string(),
                        schedule_json:
                            r#"{"kind":"daily","timeMinutes":540,"timezone":"Asia/Shanghai"}"#
                                .to_string(),
                        rrule: "FREQ=DAILY;INTERVAL=1".to_string(),
                        timezone: "Asia/Shanghai".to_string(),
                        anchor_at: 1_700_000_000_000,
                        next_run_at: Some(1_700_000_100_000),
                        notification_policy: "important_updates".to_string(),
                        target_project_snapshot: Some(project_id.to_string()),
                        target_conversation_snapshot: Some(conversation_id.to_string()),
                        target_model_snapshot: Some("model-1".to_string()),
                        target_project_id_snapshot: Some(project_id.to_string()),
                        target_conversation_id_snapshot: Some(conversation_id.to_string()),
                        target_model_id_snapshot: Some("model-1".to_string()),
                    },
                })
                .unwrap()
            {
                AutomationCreateOutcome::Created(record) => record,
                outcome => panic!("unexpected create outcome: {outcome:?}"),
            }
        };

    for (project_id, suffix) in [
        ("project-1", "referenced-conversation"),
        ("project-2", "referenced-project"),
    ] {
        let conversation_id = format!("conversation-graph-{suffix}");
        service
            .save_conversation(conversation(
                &conversation_id,
                Some(project_id),
                &format!("message-{suffix}"),
            ))
            .unwrap();
        service
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: format!("agent-graph-{suffix}"),
                conversation_id: conversation_id.clone(),
                creation_request_id: format!("ensure-agent-graph-{suffix}"),
                task_name: "Root".to_string(),
            })
            .unwrap();
        create_referencing_automation(
            &format!("automation-{suffix}"),
            &format!("automation-request-{suffix}"),
            &conversation_id,
            project_id,
        );
    }

    service
        .delete_conversation("conversation-graph-referenced-conversation")
        .unwrap();
    let conversation_blocked = service
        .get_automation("automation-referenced-conversation")
        .unwrap()
        .unwrap();
    assert_eq!(conversation_blocked.status, StoredAutomationStatus::Active);
    assert_eq!(conversation_blocked.config.health_state, "blocked");
    assert_eq!(
        conversation_blocked.config.blocked_code.as_deref(),
        Some("target_missing")
    );
    assert!(conversation_blocked.config.target_conversation_id.is_none());
    assert!(conversation_blocked.config.next_run_at.is_none());

    service.delete_project("project-2").unwrap();
    let project_blocked = service
        .get_automation("automation-referenced-project")
        .unwrap()
        .unwrap();
    assert_eq!(project_blocked.status, StoredAutomationStatus::Active);
    assert_eq!(project_blocked.config.health_state, "blocked");
    assert_eq!(
        project_blocked.config.blocked_code.as_deref(),
        Some("project_missing")
    );
    assert!(project_blocked.config.target_conversation_id.is_none());
    assert!(project_blocked.config.next_run_at.is_none());
    assert!(service
        .load_conversation("conversation-graph-referenced-project")
        .unwrap()
        .is_none());

    let connection = service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn deleting_conversation_removes_agent_rows_and_keeps_usage_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    service
        .save_conversation(conversation(
            "conversation-1",
            Some("project-1"),
            "message-1",
        ))
        .unwrap();
    service
        .upsert_agent_usage(agent_usage_record("conversation-1", "message-1"))
        .unwrap();
    service
        .store_pending_agent_action(pending_action("action-1", "conversation-1"))
        .unwrap();
    service
        .upsert_agent_action_audit(action_audit("action-1", "conversation-1"))
        .unwrap();

    service.delete_conversation("conversation-1").unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 1);
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.input_tokens, Some(12));
    assert_eq!(summary.output_tokens, Some(8));
    assert_eq!(summary.total_tokens, Some(20));

    let connection = service.state.connection().unwrap();
    let raw_usage_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
            row.get(0)
        })
        .unwrap();
    let rollup_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let pending_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_pending_actions", [], |row| {
            row.get(0)
        })
        .unwrap();
    let audit_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_action_audit", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert_eq!(raw_usage_count, 0);
    assert_eq!(rollup_count, 1);
    assert_eq!(pending_count, 0);
    assert_eq!(audit_count, 0);
}

#[test]
fn deleting_messages_keeps_usage_totals_via_rollup() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut record = conversation("conversation-usage-delete", Some("project-1"), "message-1");
    record.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "message-2".to_string(),
        role: "assistant".to_string(),
        content: "done".to_string(),
        created_at: 2,
        status: Some("sent".to_string()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
    });
    record.updated_at = 2;
    service.save_conversation(record).unwrap();
    let mut first_usage = agent_usage_record("conversation-usage-delete", "message-1");
    first_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(first_usage).unwrap();
    let mut second_usage = agent_usage_record("conversation-usage-delete", "message-2");
    second_usage.estimated_cost = Some(0.25);
    service.upsert_agent_usage(second_usage).unwrap();

    assert_eq!(
        history_timeline_projection_count(
            &service.state.connection().unwrap(),
            "conversation-usage-delete"
        ),
        2
    );

    service
        .delete_chat_messages("conversation-usage-delete", &["message-1".to_string()])
        .unwrap();

    let summary = service
        .get_usage_summary(
            &crate::AgentUsageSummaryInput {
                range: crate::AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            200_000,
        )
        .unwrap();
    assert_eq!(summary.request_count, 2);
    assert_eq!(summary.message_count, 2);
    assert_eq!(summary.input_tokens, Some(24));
    assert_eq!(summary.output_tokens, Some(16));
    assert_eq!(summary.total_tokens, Some(40));
    assert_eq!(summary.estimated_cost, Some(0.5));

    let connection = service.state.connection().unwrap();
    assert_eq!(
        history_timeline_projection_count(&connection, "conversation-usage-delete"),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_deleted_usage_daily_rollups",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn workflow_conversation_deletion_fails_only_removed_recipients_mail_and_keeps_peer_run() {
    for with_agent_tree in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        for id in ["source", "target"] {
            service
                .save_conversation(conversation(
                    id,
                    Some("project-1"),
                    &format!("message-{id}"),
                ))
                .unwrap();
        }
        if with_agent_tree {
            service
                .ensure_root_agent(&EnsureRootAgentInput {
                    agent_id: "target-agent".into(),
                    conversation_id: "target".into(),
                    creation_request_id: "target-create".into(),
                    task_name: "Target".into(),
                })
                .unwrap();
        }
        let source_version;
        let input_id;
        {
            let mut c = service.state.connection().unwrap();
            let node = |id: &str| serde_json::json!({"kind":"agent","id":id,"name":id,"x":0,"y":0,"permissionMode":"default","modelConfigId":"model","receives":"","task":"Work","delivers":""});
            let definition = serde_json::json!({"schemaVersion":1,"id":"instance","name":"Team","description":"","background":"","nodes":[node("source"),node("target")],"viewport":{"x":0,"y":0,"zoom":1}}).to_string();
            c.execute("INSERT INTO workflow_instances(instance_id,template_id,template_revision,name,color,revision,updated_at,last_request_json,definition_json) VALUES('instance','deleted-template',1,'Team','#123456',1,1,'{}',?1)",[definition]).unwrap();
            c.execute_batch("INSERT INTO workflow_instance_bindings(instance_id,node_id,conversation_id) VALUES('instance','source','source'),('instance','target','target');
                INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES('source-answer','source','assistant','',2,1);").unwrap();
            c.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES('source-answer','source','source-run',?1,'in_progress',0,2,2)",[crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION]).unwrap();
            source_version = crate::storage::workflow_execution_repository::bind_run(
                &mut c,
                "source",
                "source-run",
            )
            .unwrap()
            .unwrap()
            .execution_version;
            let receipt = crate::storage::workflow_execution_repository::send(
                &mut c,
                &crate::workflow_execution::SendRequest {
                    model_input: None,
                    recipient_versions: Default::default(),
                    conversation_id: "source".into(),
                    source_run_id: "source-run".into(),
                    tool_call_id: "send".into(),
                    execution_version: source_version.clone(),
                    messages: vec![crate::workflow_execution::SendOutput {
                        target_node_id: "target".into(),
                        message: "Only target owns this".into(),
                        reply_to_message_id: None,
                    }],
                },
            )
            .unwrap();
            input_id = receipt.input_ids[0].clone();
        }
        service.delete_conversation("target").unwrap();
        let c = service.state.connection().unwrap();
        let peer = crate::storage::workflow_execution_repository::snapshot_for_run(
            &c,
            "source",
            "source-run",
        )
        .unwrap()
        .unwrap();
        assert_eq!(peer.execution_version, source_version);
        assert!(peer.enabled);
        assert_eq!(peer.organization_revision, 2);
        let input = crate::storage::workflow_execution_repository::load_input(&c, &input_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            input.status,
            crate::workflow_execution::InputStatus::Invalidated
        );
        assert_eq!(
            input.mail_status,
            crate::workflow_execution::MailStatus::Failed
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM workflow_instance_bindings WHERE instance_id='instance'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
    }
}
