use super::*;
use crate::storage::{agent_graph_repository, migrations};
use std::sync::{Arc, Barrier};

struct Fixture {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("human-interaction.sqlite");
        let connection = Connection::open(&path).unwrap();
        migrations::run_migrations(&connection).unwrap();
        let fixture = Self { _dir: dir, path };
        let mut connection = fixture.connect();
        insert_conversation(&connection, "chat");
        agent_graph_repository::ensure_root_agent(
            &mut connection,
            &crate::EnsureRootAgentInput {
                agent_id: "root".into(),
                conversation_id: "chat".into(),
                creation_request_id: "root-create".into(),
                task_name: "Root".into(),
            },
            1,
        )
        .unwrap();
        insert_trace(&connection, "chat", "run", "assistant");
        fixture
    }
    fn connect(&self) -> Connection {
        let connection = Connection::open(&self.path).unwrap();
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        connection
    }
    fn request(&self, mode: HumanInteractionMode, tool: &str) -> HumanInteractionRequestSnapshot {
        create_request(&mut self.connect(), &owner(tool), mode, &questions(), 10).unwrap()
    }
}
fn insert_conversation(connection: &Connection, id: &str) {
    connection
        .execute(
            "INSERT INTO conversations(id,title,model_id,created_at,updated_at) VALUES(?1,?1,'model-a',1,1)",
            [id],
        )
        .unwrap();
}
fn insert_trace(connection: &Connection, conversation: &str, run: &str, assistant: &str) {
    connection.execute("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES(?1,?2,'assistant','','in_progress',2,1)", params![assistant,conversation]).unwrap();
    connection.execute("INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES(?1,?2,?3,1,'in_progress',0,2,2)", params![assistant,conversation,run]).unwrap();
}
fn owner(tool: &str) -> HostHumanInteractionOwner {
    HostHumanInteractionOwner {
        agent_id: "root".into(),
        conversation_id: "chat".into(),
        run_id: "run".into(),
        assistant_message_id: "assistant".into(),
        tool_call_id: tool.into(),
    }
}
fn questions() -> HumanInteractionToolInput {
    HumanInteractionToolInput {
        questions: vec![
            HumanInteractionQuestionInput {
                title: "Choose a color".into(),
                options: Some(vec!["Red".into(), "Blue".into()]),
            },
            HumanInteractionQuestionInput {
                title: "Any details?".into(),
                options: None,
            },
        ],
    }
}
fn submission(request: &HumanInteractionRequestSnapshot, id: &str) -> HumanInteractionSubmitInput {
    HumanInteractionSubmitInput {
        conversation_id: request.conversation_id.clone(),
        request_id: request.request_id.clone(),
        expected_revision: request.revision,
        submission_id: id.into(),
        answers: request
            .questions
            .iter()
            .map(|q| HumanInteractionAnswer::Skipped {
                question_id: q.id.clone(),
            })
            .collect(),
    }
}
fn ignored(request: &HumanInteractionRequestSnapshot, id: &str) -> HumanInteractionIgnoreInput {
    HumanInteractionIgnoreInput {
        conversation_id: request.conversation_id.clone(),
        request_id: request.request_id.clone(),
        expected_revision: request.revision,
        submission_id: id.into(),
    }
}
fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn defaults_independent_settings_cas_and_restart() {
    let fixture = Fixture::new();
    assert_eq!(
        load_settings(&fixture.connect()).unwrap(),
        HumanInteractionSettings::default()
    );
    let updated = update_settings(
        &mut fixture.connect(),
        &HumanInteractionSettingsUpdate {
            enabled: false,
            expected_revision: 0,
        },
        11,
    )
    .unwrap();
    assert_eq!(
        updated,
        HumanInteractionSettings {
            enabled: false,
            revision: 1,
            updated_at: 11
        }
    );
    assert_eq!(load_settings(&fixture.connect()).unwrap(), updated);
    assert_eq!(
        update_settings(
            &mut fixture.connect(),
            &HumanInteractionSettingsUpdate {
                enabled: true,
                expected_revision: 0
            },
            12
        )
        .unwrap_err()
        .code,
        "revision_conflict"
    );
    assert_eq!(load_settings(&fixture.connect()).unwrap(), updated);
}

#[test]
fn submit_canonical_idempotence_and_immutable_response() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "question");
    let mut input = submission(&request, "submit");
    input.answers[0] = HumanInteractionAnswer::Option {
        question_id: request.questions[0].id.clone(),
        option_id: request.questions[0].options.as_ref().unwrap()[0].id.clone(),
    };
    input.answers[1] = HumanInteractionAnswer::Text {
        question_id: request.questions[1].id.clone(),
        text: "some details".into(),
    };
    input.answers.reverse();
    let settled = submit(&mut fixture.connect(), &input, 20).unwrap();
    assert_eq!(settled.status, HumanInteractionRequestStatus::Submitted);
    assert_eq!(settled.revision, 1);
    assert_eq!(
        settled.response.as_ref().unwrap().answers[0].question_id(),
        request.questions[0].id
    );
    assert_eq!(
        settled.delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    assert_eq!(settled.delivery.as_ref().unwrap().target_run_id, None);
    input.answers.reverse();
    assert_eq!(submit(&mut fixture.connect(), &input, 40).unwrap(), settled);
    let mut changed = input.clone();
    changed.answers[0] = HumanInteractionAnswer::Skipped {
        question_id: request.questions[0].id.clone(),
    };
    assert_eq!(
        submit(&mut fixture.connect(), &changed, 40)
            .unwrap_err()
            .code,
        "conflict"
    );
    changed = input.clone();
    changed.expected_revision += 1;
    assert_eq!(
        submit(&mut fixture.connect(), &changed, 40)
            .unwrap_err()
            .code,
        "conflict"
    );
    changed = input.clone();
    changed.submission_id = "other".into();
    assert_eq!(
        submit(&mut fixture.connect(), &changed, 40)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(
        ignore(&mut fixture.connect(), &ignored(&request, "submit"), 40)
            .unwrap_err()
            .code,
        "conflict"
    );
    let connection = fixture.connect();
    assert_eq!(count(&connection, "human_interaction_responses"), 1);
    assert_eq!(count(&connection, "human_interaction_deliveries"), 1);
    assert_eq!(count(&connection, "messages"), 1);
    assert!(connection
        .execute(
            "UPDATE human_interaction_responses SET answers_json='[]'",
            []
        )
        .is_err());
    assert!(connection
        .execute("UPDATE human_interaction_requests SET run_id='foreign'", [])
        .is_err());
    assert!(connection
        .execute(
            "UPDATE human_interaction_requests SET questions_json='[]'",
            []
        )
        .is_err());
}

#[test]
fn ignore_is_async_only_idempotent_and_has_no_delivery_or_guidance() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "async");
    let input = ignored(&request, "ignore");
    let settled = ignore(&mut fixture.connect(), &input, 15).unwrap();
    assert_eq!(settled.status, HumanInteractionRequestStatus::Ignored);
    assert!(settled.response.as_ref().unwrap().answers.is_empty());
    assert!(settled.delivery.is_none());
    assert_eq!(ignore(&mut fixture.connect(), &input, 16).unwrap(), settled);
    assert_eq!(
        ignore(&mut fixture.connect(), &ignored(&request, "other"), 16)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(
        submit(&mut fixture.connect(), &submission(&request, "ignore"), 16)
            .unwrap_err()
            .code,
        "conflict"
    );
    let sync = fixture.request(HumanInteractionMode::Sync, "sync");
    assert_eq!(
        ignore(&mut fixture.connect(), &ignored(&sync, "ignore-sync"), 16)
            .unwrap_err()
            .code,
        "invalid_state"
    );
    let connection = fixture.connect();
    assert_eq!(count(&connection, "human_interaction_deliveries"), 0);
    assert_eq!(count(&connection, "agent_run_guidances"), 0);
    assert_eq!(count(&connection, "messages"), 1);
}

#[test]
fn multiple_async_but_one_open_sync_and_all_skipped_is_submission() {
    let fixture = Fixture::new();
    let sync = fixture.request(HumanInteractionMode::Sync, "sync");
    assert_eq!(
        create_request(
            &mut fixture.connect(),
            &owner("sync-two"),
            HumanInteractionMode::Sync,
            &questions(),
            10
        )
        .unwrap_err()
        .code,
        "conflict"
    );
    fixture.request(HumanInteractionMode::Async, "one");
    fixture.request(HumanInteractionMode::Async, "two");
    let settled = submit(&mut fixture.connect(), &submission(&sync, "skip-all"), 20).unwrap();
    assert_eq!(
        settled.response.as_ref().unwrap().kind,
        HumanInteractionResponseKind::Submitted
    );
    assert!(settled.delivery.is_some());
    fixture.request(HumanInteractionMode::Sync, "sync-three");
}

#[test]
fn malformed_full_responses_and_sql_failure_roll_back_every_fact() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "q");
    let original = submission(&request, "submit");
    let mut cases = Vec::new();
    let mut input = original.clone();
    input.answers.pop();
    cases.push(input);
    let mut input = original.clone();
    input.answers.push(input.answers[0].clone());
    cases.push(input);
    let mut input = original.clone();
    input.answers[1] = input.answers[0].clone();
    cases.push(input);
    let mut input = original.clone();
    input.answers[0] = HumanInteractionAnswer::Skipped {
        question_id: "foreign".into(),
    };
    cases.push(input);
    let mut input = original.clone();
    input.answers[1] = HumanInteractionAnswer::Option {
        question_id: request.questions[1].id.clone(),
        option_id: request.questions[0].options.as_ref().unwrap()[0].id.clone(),
    };
    cases.push(input);
    let mut input = original.clone();
    input.answers[0] = HumanInteractionAnswer::Text {
        question_id: request.questions[0].id.clone(),
        text: "  ".into(),
    };
    cases.push(input);
    for input in cases {
        assert_eq!(
            submit(&mut fixture.connect(), &input, 20).unwrap_err().code,
            "invalid_input"
        );
    }
    fixture.connect().execute_batch("CREATE TRIGGER fail_delivery BEFORE INSERT ON human_interaction_deliveries BEGIN SELECT RAISE(ABORT,'sensitive database path /private/example'); END;").unwrap();
    let error = submit(&mut fixture.connect(), &original, 20).unwrap_err();
    assert_eq!(error.code, "storage_unavailable");
    assert!(!error.message.contains("private"));
    let connection = fixture.connect();
    assert_eq!(
        load_request(&connection, "chat", &request.request_id).unwrap(),
        request
    );
    assert_eq!(count(&connection, "human_interaction_responses"), 0);
    assert_eq!(count(&connection, "human_interaction_deliveries"), 0);
}

#[test]
fn existing_questions_remain_answerable_after_close_and_run_completion() {
    let fixture = Fixture::new();
    let one = fixture.request(HumanInteractionMode::Async, "one");
    let two = fixture.request(HumanInteractionMode::Async, "two");
    update_settings(
        &mut fixture.connect(),
        &HumanInteractionSettingsUpdate {
            enabled: false,
            expected_revision: 0,
        },
        15,
    )
    .unwrap();
    assert_eq!(
        create_request(
            &mut fixture.connect(),
            &owner("three"),
            HumanInteractionMode::Async,
            &questions(),
            20
        )
        .unwrap_err()
        .code,
        "disabled"
    );
    fixture.connect().execute("UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=20,updated_at=20 WHERE run_id='run'",[]).unwrap();
    submit(&mut fixture.connect(), &submission(&one, "submit"), 25).unwrap();
    ignore(&mut fixture.connect(), &ignored(&two, "ignore"), 25).unwrap();
    update_settings(
        &mut fixture.connect(),
        &HumanInteractionSettingsUpdate {
            enabled: true,
            expected_revision: 1,
        },
        30,
    )
    .unwrap();
    assert_eq!(
        create_request(
            &mut fixture.connect(),
            &owner("three"),
            HumanInteractionMode::Async,
            &questions(),
            30
        )
        .unwrap_err()
        .code,
        "invalid_owner"
    );
}

#[test]
fn owner_rejects_child_foreign_conversation_run_and_assistant_bindings() {
    let fixture = Fixture::new();
    let mut connection = fixture.connect();
    insert_conversation(&connection, "foreign-chat");
    agent_graph_repository::ensure_root_agent(
        &mut connection,
        &crate::EnsureRootAgentInput {
            agent_id: "foreign-root".into(),
            conversation_id: "foreign-chat".into(),
            creation_request_id: "foreign-create".into(),
            task_name: "Root".into(),
        },
        1,
    )
    .unwrap();
    insert_trace(
        &connection,
        "foreign-chat",
        "foreign-run",
        "foreign-assistant",
    );
    let mut cases = Vec::new();
    let mut value = owner("q");
    value.agent_id = "foreign-root".into();
    cases.push(value);
    let mut value = owner("q");
    value.conversation_id = "foreign-chat".into();
    cases.push(value);
    let mut value = owner("q");
    value.run_id = "foreign-run".into();
    cases.push(value);
    let mut value = owner("q");
    value.assistant_message_id = "foreign-assistant".into();
    cases.push(value);
    for value in cases {
        assert_eq!(
            create_request(
                &mut connection,
                &value,
                HumanInteractionMode::Async,
                &questions(),
                10
            )
            .unwrap_err()
            .code,
            "invalid_owner"
        );
    }
    insert_conversation(&connection, "child-chat");
    agent_graph_repository::create_agent_node(
        &mut connection,
        &crate::CreateAgentNodeInput {
            agent_id: "child".into(),
            root_agent_id: "root".into(),
            parent_agent_id: "root".into(),
            conversation_id: "child-chat".into(),
            creation_request_id: "child-create".into(),
            task_name: "child".into(),
            task_path: "/root/child".into(),
            template_snapshot: None,
            model_snapshot: crate::AgentModelSelectionSnapshot {
                model_config_id: "model-a".into(),
                display_name: "Model".into(),
                supports_image: false,
                effective_context_window_tokens: 64000,
                model_settings_configuration_revision: "settings-v1".into(),
                provider_connection_revision: "connection-v1".into(),
                provider_protocol_revision: "protocol-v1".into(),
            },
        },
        3,
    )
    .unwrap();
    insert_trace(&connection, "child-chat", "child-run", "child-assistant");
    let child = HostHumanInteractionOwner {
        agent_id: "child".into(),
        conversation_id: "child-chat".into(),
        run_id: "child-run".into(),
        assistant_message_id: "child-assistant".into(),
        tool_call_id: "q".into(),
    };
    assert_eq!(
        create_request(
            &mut connection,
            &child,
            HumanInteractionMode::Async,
            &questions(),
            10
        )
        .unwrap_err()
        .code,
        "invalid_owner"
    );
    assert_eq!(count(&connection, "human_interaction_requests"), 0);
    let request = fixture.request(HumanInteractionMode::Async, "valid");
    let mut input = submission(&request, "submit");
    input.conversation_id = "foreign-chat".into();
    assert_eq!(
        submit(&mut connection, &input, 20).unwrap_err().code,
        "not_found"
    );
}

#[test]
fn automation_run_is_rejected_but_later_interactive_run_in_same_chat_is_allowed() {
    let fixture = Fixture::new();
    let mut connection = fixture.connect();
    connection.execute_batch("INSERT INTO automations(id,schema_version,create_request_id,title,prompt,status,health_state,destination_kind,target_conversation_id,project_binding_kind,permission_mode,permission_mode_version,permissions_json,schedule_kind,schedule_json,rrule,timezone,anchor_at,notification_policy,revision,created_at,updated_at)
        VALUES('auto',1,'auto-create','Task','Do work','active','ok','existing_chat','chat','inherit','default',1,'{}','interval','{}','FREQ=HOURLY','UTC',0,'all_runs',1,1,1);
        INSERT INTO automation_runs(id,schema_version,automation_id,config_revision,config_snapshot_json,trigger_kind,scheduled_for,status,agent_run_id,conversation_id,assistant_message_id,created_at,updated_at,started_at)
        VALUES('auto-run',1,'auto',1,'{}','scheduled',1,'running','run','chat','assistant',2,2,2);").unwrap();
    assert_eq!(
        create_request(
            &mut connection,
            &owner("q"),
            HumanInteractionMode::Async,
            &questions(),
            10
        )
        .unwrap_err()
        .code,
        "invalid_owner"
    );
    connection.execute("UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=20,updated_at=20 WHERE run_id='run'",[]).unwrap();
    insert_trace(&connection, "chat", "human-run", "human-assistant");
    let mut human = owner("q");
    human.run_id = "human-run".into();
    human.assistant_message_id = "human-assistant".into();
    assert!(create_request(
        &mut connection,
        &human,
        HumanInteractionMode::Async,
        &questions(),
        30
    )
    .is_ok());
}

#[test]
fn concurrent_settings_compare_and_swap_has_one_winner() {
    let fixture = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let mut connection = fixture.connect();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                update_settings(
                    &mut connection,
                    &HumanInteractionSettingsUpdate {
                        enabled: false,
                        expected_revision: 0,
                    },
                    10,
                )
            })
        })
        .collect();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|value| value.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .find_map(|value| value.as_ref().err())
            .unwrap()
            .code,
        "revision_conflict"
    );
    assert_eq!(load_settings(&fixture.connect()).unwrap().revision, 1);
}

#[test]
fn submit_ignore_concurrency_has_one_durable_winner() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "q");
    let barrier = Arc::new(Barrier::new(2));
    let mut submit_connection = fixture.connect();
    let mut ignore_connection = fixture.connect();
    let submit_input = submission(&request, "submit");
    let ignore_input = ignored(&request, "ignore");
    let worker_barrier = barrier.clone();
    let worker = std::thread::spawn(move || {
        worker_barrier.wait();
        submit(&mut submit_connection, &submit_input, 20)
    });
    barrier.wait();
    let ignored = ignore(&mut ignore_connection, &ignore_input, 20);
    let submitted = worker.join().unwrap();
    assert_ne!(submitted.is_ok(), ignored.is_ok());
    let winner = submitted.or(ignored).unwrap();
    let connection = fixture.connect();
    assert_eq!(
        load_request(&connection, "chat", &request.request_id).unwrap(),
        winner
    );
    assert_eq!(count(&connection, "human_interaction_responses"), 1);
    assert_eq!(
        count(&connection, "human_interaction_deliveries"),
        if winner.status == HumanInteractionRequestStatus::Submitted {
            1
        } else {
            0
        }
    );
    assert_eq!(count(&connection, "messages"), 1);
}

#[test]
fn concurrent_same_submission_replays_exactly_one_response() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "q");
    let input = submission(&request, "same");
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let mut connection = fixture.connect();
            let barrier = barrier.clone();
            let input = input.clone();
            std::thread::spawn(move || {
                barrier.wait();
                submit(&mut connection, &input, 20)
            })
        })
        .collect();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap().unwrap())
        .collect();
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(count(&fixture.connect(), "human_interaction_responses"), 1);
    assert_eq!(count(&fixture.connect(), "human_interaction_deliveries"), 1);
}

#[test]
fn close_setting_and_question_creation_share_one_write_order() {
    // Independent SQLite connections exercise the database boundary, not a process mutex.
    for _ in 0..4 {
        let fixture = Fixture::new();
        let barrier = Arc::new(Barrier::new(2));
        let mut close_connection = fixture.connect();
        let mut create_connection = fixture.connect();
        let worker_barrier = barrier.clone();
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            update_settings(
                &mut close_connection,
                &HumanInteractionSettingsUpdate {
                    enabled: false,
                    expected_revision: 0,
                },
                20,
            )
        });
        barrier.wait();
        let created = create_request(
            &mut create_connection,
            &owner("q"),
            HumanInteractionMode::Async,
            &questions(),
            20,
        );
        let setting = worker.join().unwrap().unwrap();
        assert!(!setting.enabled);
        match created {
            Ok(request) => {
                assert_eq!(request.policy_revision, 0);
                assert_eq!(count(&fixture.connect(), "human_interaction_requests"), 1);
            }
            Err(error) => {
                assert_eq!(error.code, "disabled");
                assert_eq!(count(&fixture.connect(), "human_interaction_requests"), 0);
            }
        }
        assert_eq!(
            create_request(
                &mut create_connection,
                &owner("after-close"),
                HumanInteractionMode::Async,
                &questions(),
                30
            )
            .unwrap_err()
            .code,
            "disabled"
        );
    }
}

#[test]
fn durable_paging_cursor_scope_validation_and_response_restart() {
    let fixture = Fixture::new();
    let mut all = Vec::new();
    for n in 0..5 {
        all.push(fixture.request(HumanInteractionMode::Async, &format!("q-{n}")));
    }
    assert!(all.iter().all(|request| request.created_at == 10));
    assert!(all
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    let settled = submit(&mut fixture.connect(), &submission(&all[0], "submit"), 20).unwrap();
    let mut cursor = None;
    let mut ids = std::collections::BTreeSet::new();
    let mut ordered_ids = Vec::new();
    loop {
        let page = list_requests(
            &mut fixture.connect(),
            &HumanInteractionListInput {
                conversation_id: "chat".into(),
                cursor: cursor.clone(),
                limit: 2,
            },
        )
        .unwrap();
        for item in page.items {
            if item.request_id == settled.request_id {
                assert_eq!(item, settled);
            }
            ordered_ids.push(item.request_id.clone());
            assert!(ids.insert(item.request_id));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 5);
    assert_eq!(
        ordered_ids,
        all.iter()
            .rev()
            .map(|request| request.request_id.clone())
            .collect::<Vec<_>>()
    );
    let page = list_requests(
        &mut fixture.connect(),
        &HumanInteractionListInput {
            conversation_id: "chat".into(),
            cursor: None,
            limit: 1,
        },
    )
    .unwrap();
    for input in [
        HumanInteractionListInput {
            conversation_id: "chat".into(),
            cursor: Some("x".repeat(2049)),
            limit: 1,
        },
        HumanInteractionListInput {
            conversation_id: "chat".into(),
            cursor: Some("invalid!".into()),
            limit: 1,
        },
        HumanInteractionListInput {
            conversation_id: "foreign".into(),
            cursor: page.next_cursor,
            limit: 1,
        },
        HumanInteractionListInput {
            conversation_id: "chat".into(),
            cursor: None,
            limit: 0,
        },
        HumanInteractionListInput {
            conversation_id: "chat".into(),
            cursor: None,
            limit: 101,
        },
    ] {
        assert_eq!(
            list_requests(&mut fixture.connect(), &input)
                .unwrap_err()
                .code,
            "invalid_input"
        );
    }
    for sequence in [0, HUMAN_INTERACTION_MAX_SAFE_INTEGER + 1] {
        let cursor = URL_SAFE_NO_PAD.encode(
            json(&ListCursor {
                version: 1,
                conversation_id: "chat".into(),
                sequence,
            })
            .unwrap(),
        );
        assert_eq!(
            list_requests(
                &mut fixture.connect(),
                &HumanInteractionListInput {
                    conversation_id: "chat".into(),
                    cursor: Some(cursor),
                    limit: 1
                }
            )
            .unwrap_err()
            .code,
            "invalid_input"
        );
    }
}

#[test]
fn private_suspension_is_bound_durable_opaque_and_never_in_public_snapshot() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Sync, "sync");
    let private = HumanInteractionSuspension {
        request_id: request.request_id.clone(),
        run_id: request.run_id.clone(),
        assistant_message_id: request.assistant_message_id.clone(),
        tool_call_id: request.tool_call_id.clone(),
        checkpoint: serde_json::json!({"opaqueFutureRuntimeMaterial":"private-checkpoint"}),
    };
    save_suspension(&mut fixture.connect(), &private, 20).unwrap();
    save_suspension(&mut fixture.connect(), &private, 30).unwrap();
    assert!(
        load_suspension(&fixture.connect(), &request.request_id).unwrap() == Some(private.clone())
    );
    let public = load_request(&fixture.connect(), "chat", &request.request_id).unwrap();
    assert_eq!(public, request);
    assert!(!json(&public).unwrap().contains("private-checkpoint"));
    let mut bad = private.clone();
    bad.run_id = "foreign".into();
    assert_eq!(
        save_suspension(&mut fixture.connect(), &bad, 30)
            .unwrap_err()
            .code,
        "conflict"
    );
    let mut bad = private.clone();
    bad.checkpoint = serde_json::json!([]);
    assert_eq!(
        save_suspension(&mut fixture.connect(), &bad, 30)
            .unwrap_err()
            .code,
        "invalid_input"
    );
    let mut bad = private.clone();
    bad.checkpoint = serde_json::json!({"different":true});
    assert_eq!(
        save_suspension(&mut fixture.connect(), &bad, 30)
            .unwrap_err()
            .code,
        "conflict"
    );
    let async_request = fixture.request(HumanInteractionMode::Async, "async");
    let mut bad = private.clone();
    bad.request_id = async_request.request_id;
    bad.tool_call_id = async_request.tool_call_id;
    assert_eq!(
        save_suspension(&mut fixture.connect(), &bad, 30)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(
        count(&fixture.connect(), "human_interaction_suspensions"),
        1
    );
}

#[test]
fn no_question_count_cap_but_generated_ids_must_fit_a_complete_response() {
    let fixture = Fixture::new();
    let many = HumanInteractionToolInput {
        questions: (0..200)
            .map(|_| HumanInteractionQuestionInput {
                title: "Q".into(),
                options: None,
            })
            .collect(),
    };
    let request = create_request(
        &mut fixture.connect(),
        &owner("many"),
        HumanInteractionMode::Async,
        &many,
        10,
    )
    .unwrap();
    assert_eq!(request.questions.len(), 200);
    submit(
        &mut fixture.connect(),
        &submission(&request, "all-skipped"),
        20,
    )
    .unwrap();
    let impossible = HumanInteractionToolInput {
        questions: (0..5000)
            .map(|_| HumanInteractionQuestionInput {
                title: "Q".into(),
                options: None,
            })
            .collect(),
    };
    validate_human_interaction_tool_input(&impossible).unwrap();
    assert_eq!(
        create_request(
            &mut fixture.connect(),
            &owner("too-large"),
            HumanInteractionMode::Async,
            &impossible,
            10
        )
        .unwrap_err()
        .code,
        "invalid_input"
    );
    assert_eq!(count(&fixture.connect(), "human_interaction_requests"), 1);
}

#[test]
fn corrupt_persisted_response_delivery_combinations_are_sanitized() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "submitted");
    submit(&mut fixture.connect(), &submission(&request, "submit"), 20).unwrap();
    fixture
        .connect()
        .execute("DELETE FROM human_interaction_deliveries", [])
        .unwrap();
    assert_eq!(
        load_request(&fixture.connect(), "chat", &request.request_id)
            .unwrap_err()
            .code,
        "storage_unavailable"
    );
    let request = fixture.request(HumanInteractionMode::Async, "ignored");
    let ignored = ignore(&mut fixture.connect(), &ignored(&request, "ignore"), 20).unwrap();
    fixture.connect().execute("INSERT INTO human_interaction_deliveries(response_id,status,revision) VALUES(?1,'pending',0)", [&ignored.response.unwrap().response_id]).unwrap();
    assert_eq!(
        load_request(&fixture.connect(), "chat", &request.request_id)
            .unwrap_err()
            .code,
        "storage_unavailable"
    );
}

#[path = "sync_tests.rs"]
mod sync_tests;

#[path = "async_tests.rs"]
mod async_tests;
