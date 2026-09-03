use super::*;

#[test]
fn run_bound_spawn_is_rejected_atomically_after_durable_tree_stop() {
    let fixture = Fixture::new(Some("model-a"));
    insert_active_root_trace(&fixture, "run-stopped-spawn", "assistant-stopped-spawn");
    fixture
        .service
        .begin_agent_tree_run_stop_by_root_conversation_at(
            "root-conversation",
            "run-stopped-spawn",
            21,
        )
        .unwrap()
        .unwrap();

    let error = fixture
        .service
        .create_child_agent_with_limits_and_expected_selector_from_run(
            &spawn_input("spawn-after-stop", "blocked_child"),
            AgentTreeResourceLimits::default(),
            None,
            None,
            "run-stopped-spawn",
        )
        .unwrap_err();
    assert_eq!(
        error,
        ChildAgentSpawnError::Conflict("origin Agent Run has been stopped".to_string())
    );

    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0,
        "the rejected Spawn must not leave a child, Conversation, task, or Wake"
    );
}

#[test]
fn retry_uses_frozen_bundle_after_template_and_model_catalog_change() {
    let fixture = Fixture::new(Some("model-a"));
    let mut input = spawn_input("spawn-frozen-retry", "frozen_review");
    input.template_machine_key = Some("reviewer".to_string());
    let created = fixture.service.create_child_agent(&input).unwrap();
    let template = fixture
        .service
        .get_agent_template("template-reviewer")
        .unwrap();
    fixture
        .service
        .set_agent_template_enabled("template-reviewer", template.revision, false)
        .unwrap();
    fixture
        .service
        .save_model_settings(model_settings(vec![model("model-a", true)]))
        .unwrap();

    let retry = fixture.service.create_child_agent(&input).unwrap();
    assert_eq!(retry, created);
    assert_eq!(
        retry.agent.model_snapshot.unwrap().model_config_id,
        "model-b"
    );
}

#[test]
fn removing_project_assignment_blocks_new_spawns_but_not_idempotent_retry() {
    let fixture = Fixture::new(Some("model-a"));
    let mut first = spawn_input("spawn-before-unassign", "before_unassign");
    first.template_machine_key = Some("reviewer".to_string());
    let created = fixture.service.create_child_agent(&first).unwrap();

    fixture
        .service
        .set_agent_template_project_assignment("project-a", "template-reviewer", false)
        .unwrap();

    assert_eq!(fixture.service.create_child_agent(&first).unwrap(), created);
    let mut later = spawn_input("spawn-after-unassign", "after_unassign");
    later.template_machine_key = Some("reviewer".to_string());
    assert_eq!(
        fixture.service.create_child_agent(&later).unwrap_err(),
        ChildAgentSpawnError::TemplateNotFound("reviewer".to_string())
    );
}

#[test]
fn frozen_selector_identity_rejects_a_template_revised_during_the_turn() {
    let fixture = Fixture::new(Some("model-a"));
    let original = fixture
        .service
        .get_agent_template("template-reviewer")
        .unwrap();
    fixture
        .service
        .update_agent_template(&UpdateAgentTemplateInput {
            template_id: original.template_id.clone(),
            expected_revision: original.revision,
            name: original.name,
            description: "Updated after the selector directory was frozen".to_string(),
            instructions: original.instructions,
            model_config_id: original.model_config_id,
        })
        .unwrap();

    let mut input = spawn_input("spawn-stale-selector", "stale_selector");
    input.template_machine_key = Some("reviewer".to_string());
    assert_eq!(
        fixture
            .service
            .create_child_agent_with_limits_and_expected_selector(
                &input,
                AgentTreeResourceLimits::default(),
                None,
                Some(("template-reviewer", original.revision)),
            )
            .unwrap_err(),
        ChildAgentSpawnError::Conflict(
            "selected Agent template changed after this Turn began".to_string()
        )
    );
}

#[test]
fn model_priority_is_explicit_then_template_then_parent_then_ordered_default() {
    let fixture = Fixture::new(Some("model-a"));
    let mut explicit = spawn_input("spawn-explicit", "explicit_review");
    explicit.template_machine_key = Some("reviewer".to_string());
    explicit.explicit_model_id = Some("model-c".to_string());
    let explicit = fixture.service.create_child_agent(&explicit).unwrap();
    assert_eq!(
        explicit.model_selection_source,
        AgentModelSelectionSource::Explicit
    );
    assert_eq!(
        explicit.agent.model_snapshot.unwrap().model_config_id,
        "model-c"
    );
    assert_eq!(
        explicit.agent.template_snapshot.unwrap().machine_key,
        "reviewer"
    );

    let mut template = spawn_input("spawn-template", "template_review");
    template.template_machine_key = Some("reviewer".to_string());
    let template = fixture.service.create_child_agent(&template).unwrap();
    assert_eq!(
        template.model_selection_source,
        AgentModelSelectionSource::Template
    );
    assert_eq!(
        template.agent.model_snapshot.unwrap().model_config_id,
        "model-b"
    );

    let parent = fixture
        .service
        .create_child_agent(&spawn_input("spawn-parent", "parent_review"))
        .unwrap();
    assert_eq!(
        parent.model_selection_source,
        AgentModelSelectionSource::Parent
    );
    assert_eq!(
        parent.agent.model_snapshot.unwrap().model_config_id,
        "model-a"
    );

    let default_fixture = Fixture::new(None);
    let default = default_fixture
        .service
        .create_child_agent(&spawn_input("spawn-default", "default_review"))
        .unwrap();
    assert_eq!(
        default.model_selection_source,
        AgentModelSelectionSource::Default
    );
    assert_eq!(
        default.agent.model_snapshot.unwrap().model_config_id,
        "model-a"
    );
}

#[test]
fn exact_unavailable_model_and_unsupported_reasoning_never_fall_back() {
    let fixture = Fixture::new(Some("model-a"));
    let mut missing = spawn_input("spawn-missing", "missing_model");
    missing.explicit_model_id = Some("does-not-exist".to_string());
    assert_eq!(
        fixture.service.create_child_agent(&missing).unwrap_err(),
        ChildAgentSpawnError::ModelUnavailable {
            model_config_id: Some("does-not-exist".to_string()),
            reason: crate::AgentModelUnavailableReason::NotFound,
        }
    );

    let mut reasoning = spawn_input("spawn-reasoning", "reasoning_override");
    reasoning.reasoning_effort = Some(ReasoningEffort::High);
    assert_eq!(
        fixture.service.create_child_agent(&reasoning).unwrap_err(),
        ChildAgentSpawnError::UnsupportedReasoningEffort(ReasoningEffort::High)
    );
    assert!(fixture
        .service
        .list_agent_children("agent-root", "agent-root")
        .unwrap()
        .is_empty());
}

#[test]
fn exact_enabled_deepseek_reasoning_is_frozen_for_high_and_max() {
    for (index, effort) in [ReasoningEffort::High, ReasoningEffort::Max]
        .into_iter()
        .enumerate()
    {
        let fixture = Fixture::new(Some("model-a"));
        fixture
            .service
            .save_model_settings(model_settings(vec![deepseek_model(
                "model-a",
                true,
                crate::ReasoningMode::Enabled,
                effort,
            )]))
            .unwrap();
        let mut input = spawn_input(
            &format!("spawn-deepseek-reasoning-{index}"),
            &format!("deepseek_reasoning_{index}"),
        );
        input.reasoning_effort = Some(effort);
        let spawn = fixture.service.create_child_agent(&input).unwrap();
        assert_eq!(spawn.agent.reasoning_effort_snapshot, Some(effort));
    }
}

#[test]
fn v2_deepseek_reasoning_is_frozen_for_exact_legacy_agent_efforts() {
    for (index, (requested, configured)) in [
        (ReasoningEffort::High, ProviderReasoningEffort::High),
        (ReasoningEffort::Max, ProviderReasoningEffort::Max),
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = Fixture::new(Some("model-a"));
        fixture
            .service
            .save_model_settings(model_settings(vec![deepseek_v2_model(
                "deepseek-v4-flash",
                true,
                crate::ReasoningMode::Enabled,
                configured,
            )]))
            .unwrap();
        let mut input = spawn_input(
            &format!("spawn-v2-deepseek-reasoning-{index}"),
            &format!("v2_deepseek_reasoning_{index}"),
        );
        input.explicit_model_id = Some("deepseek-v4-flash".to_string());
        input.reasoning_effort = Some(requested);

        let spawn = fixture.service.create_child_agent(&input).unwrap();
        assert_eq!(spawn.agent.reasoning_effort_snapshot, Some(requested));
    }
}

#[test]
fn v2_low_reasoning_never_enters_the_legacy_agent_effort_snapshot() {
    let fixture = Fixture::new(Some("model-a"));
    fixture
        .service
        .save_model_settings(model_settings(vec![deepseek_v2_model(
            "deepseek-v4-flash",
            true,
            crate::ReasoningMode::Enabled,
            ProviderReasoningEffort::Low,
        )]))
        .unwrap();

    for (index, requested) in [ReasoningEffort::High, ReasoningEffort::Max]
        .into_iter()
        .enumerate()
    {
        let mut input = spawn_input(
            &format!("spawn-v2-low-rejected-{index}"),
            &format!("v2_low_rejected_{index}"),
        );
        input.explicit_model_id = Some("deepseek-v4-flash".to_string());
        input.reasoning_effort = Some(requested);
        assert_eq!(
            fixture.service.create_child_agent(&input).unwrap_err(),
            ChildAgentSpawnError::UnsupportedReasoningEffort(requested)
        );
    }
    assert!(fixture
        .service
        .list_agent_children("agent-root", "agent-root")
        .unwrap()
        .is_empty());
}

#[test]
fn provider_default_disabled_and_mismatched_reasoning_are_rejected() {
    for (index, mode, configured, requested) in [
        (
            0,
            crate::ReasoningMode::Enabled,
            ReasoningEffort::High,
            ReasoningEffort::ProviderDefault,
        ),
        (
            1,
            crate::ReasoningMode::Disabled,
            ReasoningEffort::ProviderDefault,
            ReasoningEffort::High,
        ),
        (
            2,
            crate::ReasoningMode::Enabled,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ),
    ] {
        let fixture = Fixture::new(Some("model-a"));
        fixture
            .service
            .save_model_settings(model_settings(vec![deepseek_model(
                "model-a", true, mode, configured,
            )]))
            .unwrap();
        let mut input = spawn_input(
            &format!("spawn-rejected-reasoning-{index}"),
            &format!("rejected_reasoning_{index}"),
        );
        input.reasoning_effort = Some(requested);
        assert_eq!(
            fixture.service.create_child_agent(&input).unwrap_err(),
            ChildAgentSpawnError::UnsupportedReasoningEffort(requested)
        );
    }
}

#[test]
fn reasoning_retry_is_frozen_but_wake_fails_closed_after_profile_drift() {
    let fixture = Fixture::new(Some("model-a"));
    fixture
        .service
        .save_model_settings(model_settings(vec![deepseek_model(
            "model-a",
            true,
            crate::ReasoningMode::Enabled,
            ReasoningEffort::High,
        )]))
        .unwrap();
    let mut input = spawn_input("spawn-reasoning-drift", "reasoning_drift");
    input.reasoning_effort = Some(ReasoningEffort::High);
    let created = fixture.service.create_child_agent(&input).unwrap();

    fixture
        .service
        .save_model_settings(model_settings(vec![deepseek_model(
            "model-a",
            true,
            crate::ReasoningMode::Enabled,
            ReasoningEffort::Max,
        )]))
        .unwrap();
    assert_eq!(fixture.service.create_child_agent(&input).unwrap(), created);

    let claim_token = "reasoning-drift-claim";
    fixture
        .service
        .claim_next_agent_wake(&created.agent.agent_id, claim_token)
        .unwrap()
        .unwrap();
    fixture
        .service
        .transition_agent_wake(
            &created.initial_wake.wake_id,
            AgentWakeStatus::Claimed,
            AgentWakeStatus::Running,
            Some(claim_token),
        )
        .unwrap();
    assert!(matches!(
        fixture.service.resolve_running_child_agent_wake(
            &created.agent.agent_id,
            &created.initial_wake.wake_id,
            &created.task_message.message_id,
            claim_token,
        ),
        Err(AgentGraphError::Conflict(reason))
            if reason.contains("reasoning constraint")
    ));
}

#[test]
fn failure_after_projection_rolls_back_every_spawn_fact() {
    let fixture = Fixture::new(Some("model-a"));
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_child_initial_wake
                     BEFORE INSERT ON agent_wake_requests
                     BEGIN
                         SELECT RAISE(ABORT, 'injected child wake failure');
                     END;",
            )
            .unwrap();
    }
    assert!(matches!(
        fixture
            .service
            .create_child_agent(&spawn_input("spawn-rollback", "rollback_review")),
        Err(ChildAgentSpawnError::StorageUnavailable(_))
    ));
    let connection = fixture.service.state.connection().unwrap();
    for table in [
        "agent_nodes",
        "agent_mailbox_messages",
        "agent_wake_requests",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE root_agent_id = 'agent-root'");
        let count = connection
            .query_row(&sql, [], |row| row.get::<_, u64>(0))
            .unwrap();
        assert_eq!(count, u64::from(table == "agent_nodes"), "{table}");
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn projection_failure_rolls_back_conversation_node_and_mailbox() {
    let fixture = Fixture::new(Some("model-a"));
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TRIGGER fail_child_task_projection
                     BEFORE INSERT ON messages
                     WHEN NEW.input_origin_kind = 'agent'
                     BEGIN
                         SELECT RAISE(ABORT, 'injected child projection failure');
                     END;",
            )
            .unwrap();
    }
    assert!(matches!(
        fixture
            .service
            .create_child_agent(&spawn_input("spawn-projection-fail", "projection_failure")),
        Err(ChildAgentSpawnError::StorageUnavailable(_))
    ));
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn incomplete_parent_model_context_rejects_spawn_and_rolls_back_every_child_fact() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute(
                "DELETE FROM conversation_turn_traces
                     WHERE assistant_message_id = 'root-assistant-0'",
                [],
            )
            .unwrap();
        conversation_trace_repository::commit_trace_in_connection(
            &connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "root-run-0".to_string(),
                conversation_id: "root-conversation".to_string(),
                assistant_message_id: "root-assistant-0".to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![crate::ConversationTurnTraceItem::AssistantNarration {
                    sequence: 0,
                    content: "Inspecting the parent history.".to_string(),
                    truncated: false,
                }],
            },
            11,
            12,
        )
        .unwrap();
    }

    let mut input = spawn_input("spawn-incomplete-context", "incomplete_context");
    input.fork_turns = AgentForkTurns::All;
    let error = fixture.service.create_child_agent(&input).unwrap_err();
    assert!(matches!(
        &error,
        ChildAgentSpawnError::SnapshotUnavailable(message)
            if message.contains("incomplete model context")
    ));

    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM agent_nodes WHERE parent_agent_id IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id != 'root-conversation'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM child_context_snapshots", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_mailbox_messages", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_wake_requests", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn atomic_tree_limits_preserve_idempotent_retry_and_reject_depth_nodes_and_task_bytes() {
    let fixture = Fixture::new(Some("model-a"));
    let limits = AgentTreeResourceLimits {
        max_depth: 2,
        max_nodes: 2,
        max_task_bytes: 64,
    };
    let first_input = spawn_input("spawn-limited-first", "first");
    let first = fixture
        .service
        .create_child_agent_with_limits(&first_input, limits)
        .unwrap();
    let retry = fixture
        .service
        .create_child_agent_with_limits(
            &first_input,
            AgentTreeResourceLimits {
                max_depth: 1,
                max_nodes: 2,
                max_task_bytes: 1,
            },
        )
        .unwrap();
    assert_eq!(retry.agent.agent_id, first.agent.agent_id);

    let node_error = fixture
        .service
        .create_child_agent_with_limits(&spawn_input("spawn-limited-second", "second"), limits)
        .unwrap_err();
    assert_eq!(
        node_error,
        ChildAgentSpawnError::ResourceLimit {
            resource: "tree_nodes",
            limit: 2,
        }
    );

    let depth_fixture = Fixture::new(Some("model-a"));
    let child = depth_fixture
        .service
        .create_child_agent_with_limits(
            &spawn_input("spawn-depth-parent", "depth_parent"),
            AgentTreeResourceLimits {
                max_depth: 1,
                max_nodes: 8,
                max_task_bytes: 64,
            },
        )
        .unwrap();
    let mut grandchild_input = spawn_input("spawn-depth-child", "depth_child");
    grandchild_input.parent_agent_id = child.agent.agent_id;
    let depth_error = depth_fixture
        .service
        .create_child_agent_with_limits(
            &grandchild_input,
            AgentTreeResourceLimits {
                max_depth: 1,
                max_nodes: 8,
                max_task_bytes: 64,
            },
        )
        .unwrap_err();
    assert_eq!(
        depth_error,
        ChildAgentSpawnError::ResourceLimit {
            resource: "tree_depth",
            limit: 1,
        }
    );

    let mut oversized = spawn_input("spawn-oversized-task", "oversized");
    oversized.task = "x".repeat(65);
    assert!(matches!(
        depth_fixture.service.create_child_agent_with_limits(
            &oversized,
            AgentTreeResourceLimits {
                max_depth: 2,
                max_nodes: 8,
                max_task_bytes: 64,
            },
        ),
        Err(ChildAgentSpawnError::ResourceLimit {
            resource: "task_bytes",
            limit: 64,
        })
    ));
}

/// Deterministic release profile for the collaboration persistence boundary.
///
/// Kept ignored in the ordinary unit suite because it intentionally performs at least ten
/// thousand durable SQLite commits. The repository release-gate script runs it explicitly and
/// rejects environment overrides below the documented minimums.
#[test]
#[ignore = "run with pnpm test:multi-agent-release"]
fn release_profile_tree_mailbox_event_contention_and_restart_recovery() {
    const CONFIGURED_TREE_NODES: usize = 64;
    const WRITERS: usize = 4;
    let fact_count = release_profile_usize("MYCOPILOT_MULTI_AGENT_PROFILE_FACTS", 10_000);
    let restart_count = release_profile_usize("MYCOPILOT_MULTI_AGENT_PROFILE_RESTARTS", 20);
    assert!(
        fact_count >= 10_000,
        "release profile requires >=10000 facts"
    );
    assert!(
        restart_count >= 20,
        "release profile requires >=20 restarts"
    );

    let profile_started = Instant::now();
    let fixture = Fixture::new(Some("model-a"));
    let database_path = fixture._directory.path().join("storage.sqlite");
    let limits = AgentTreeResourceLimits::default();
    assert_eq!(limits.max_nodes as usize, CONFIGURED_TREE_NODES);

    let mut children = Vec::with_capacity(CONFIGURED_TREE_NODES - 1);
    for index in 0..(CONFIGURED_TREE_NODES - 1) {
        let created = fixture
            .service
            .create_child_agent_with_limits(
                &spawn_input(
                    &format!("release-spawn-{index}"),
                    &format!("release_child_{index:02}"),
                ),
                limits,
            )
            .unwrap();
        children.push(created);
    }
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        64
    );
    assert_eq!(
        fixture
            .service
            .create_child_agent_with_limits(
                &spawn_input("release-spawn-over-limit", "release_child_over_limit"),
                limits,
            )
            .unwrap_err(),
        ChildAgentSpawnError::ResourceLimit {
            resource: "tree_nodes",
            limit: 64,
        }
    );

    // Settle every initial Wake through the durable result/outbox transaction. The root stays
    // idle: results are pending Mailbox facts and never create a root Wake.
    for (index, child) in children.iter().enumerate() {
        let token = format!("release-claim-{index}");
        let claimed = fixture
            .service
            .claim_next_agent_wake(&child.agent.agent_id, &token)
            .unwrap()
            .unwrap();
        fixture
            .service
            .transition_agent_wake(
                &claimed.wake_id,
                AgentWakeStatus::Claimed,
                AgentWakeStatus::Running,
                Some(&token),
            )
            .unwrap();
        fixture
            .service
            .finish_agent_wake_with_result(&FinishAgentWakeWithResultInput {
                wake_id: claimed.wake_id,
                expected_status: AgentWakeStatus::Running,
                claim_token: token,
                terminal_status: AgentWakeStatus::Completed,
                terminal_error: None,
                result_message: EnqueueAgentMessageInput {
                    message_id: format!("release-result-{index}"),
                    root_agent_id: "agent-root".to_string(),
                    sender_agent_id: child.agent.agent_id.clone(),
                    recipient_agent_id: "agent-root".to_string(),
                    request_id: format!("release-result-request-{index}"),
                    kind: AgentMailboxKind::Result,
                    content: format!("result {index}"),
                    projection_message_id: format!("release-result-projection-{index}"),
                },
            })
            .unwrap();
    }

    let child_ids = Arc::new(
        children
            .iter()
            .map(|child| child.agent.agent_id.clone())
            .collect::<Vec<_>>(),
    );
    let errors = Arc::new(AtomicUsize::new(0));
    let busy_errors = Arc::new(AtomicUsize::new(0));
    let maximum_enqueue_micros = Arc::new(AtomicU64::new(0));
    std::thread::scope(|scope| {
        for writer_index in 0..WRITERS {
            // Production serializes writes through StorageState's SQLite mutex. Four caller
            // threads still exercise the public contention boundary without inventing a
            // second in-process database owner that Core Server never creates.
            let service = &fixture.service;
            let child_ids = Arc::clone(&child_ids);
            let errors = Arc::clone(&errors);
            let busy_errors = Arc::clone(&busy_errors);
            let maximum_enqueue_micros = Arc::clone(&maximum_enqueue_micros);
            scope.spawn(move || {
                    for sequence in (writer_index..fact_count).step_by(WRITERS) {
                        let input = SendAgentMessageRequest {
                            sender_agent_id: "agent-root".to_string(),
                            recipient_agent_id: child_ids[sequence % child_ids.len()].clone(),
                            request_id: format!("release-message-{sequence}"),
                            content: format!("release payload {sequence}"),
                        };
                        let started = Instant::now();
                        if let Err(error) = service.send_agent_message(&input) {
                            errors.fetch_add(1, Ordering::Relaxed);
                            if matches!(&error, AgentGraphError::StorageUnavailable(message) if message.contains("busy") || message.contains("locked"))
                            {
                                busy_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        maximum_enqueue_micros.fetch_max(
                            started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                    }
                });
        }
    });
    assert_eq!(
        errors.load(Ordering::Relaxed),
        0,
        "all durable sends converge (sqlite_busy_errors={})",
        busy_errors.load(Ordering::Relaxed)
    );
    assert_eq!(
        busy_errors.load(Ordering::Relaxed),
        0,
        "no SQLITE_BUSY escapes"
    );

    // Retry a deterministic sample after the concurrent writers have committed. Idempotency
    // returns the original fact and does not advance the event sequence.
    for sequence in (0..fact_count).step_by(997) {
        let dispatch = fixture
            .service
            .send_agent_message(&SendAgentMessageRequest {
                sender_agent_id: "agent-root".to_string(),
                recipient_agent_id: child_ids[sequence % child_ids.len()].clone(),
                request_id: format!("release-message-{sequence}"),
                content: format!("release payload {sequence}"),
            })
            .unwrap();
        assert_eq!(
            dispatch.message.request_id,
            format!("release-message-{sequence}")
        );
    }

    // Twenty deterministic pre-dispatch crashes. Each cycle leaves one claimed Wake behind,
    // opens a new StorageService (the process-restart boundary), advances the fake recovery
    // clock past the half-open lease, reclaims the same durable Wake, and cancels it without a
    // second Turn. This is deliberately stronger than repeatedly opening an idle database.
    let restart_clock_base = now_ms();
    let mut maximum_restart_micros = 0_u128;
    for restart in 0..restart_count {
        let target = &child_ids[restart % child_ids.len()];
        fixture
            .service
            .follow_up_agent(&SendAgentMessageRequest {
                sender_agent_id: "agent-root".to_string(),
                recipient_agent_id: target.clone(),
                request_id: format!("release-restart-followup-{restart}"),
                content: format!("recover this durable follow-up {restart}"),
            })
            .unwrap();
        let claimed_at = restart_clock_base + (restart as i64 * 100_000);
        let stale_claim = format!("release-stale-claim-{restart}");
        let claimed = fixture
            .service
            .claim_next_dispatchable_agent_wake_at(&stale_claim, claimed_at)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.agent_id, *target);

        let started = Instant::now();
        let reopened = StorageService::open(&database_path).unwrap();
        let recovered_at = claimed_at + 60_001;
        let recovered = reopened
            .recover_agent_wakes_at(&format!("release-recovery-{restart}"), recovered_at)
            .unwrap();
        assert_eq!(recovered.requeued_before_dispatch, 1);
        assert!(recovered.actions.is_empty());
        let retry_claim = format!("release-retry-claim-{restart}");
        let retry = reopened
            .claim_next_dispatchable_agent_wake_at(&retry_claim, recovered_at + 1)
            .unwrap()
            .unwrap();
        assert_eq!(retry.wake_id, claimed.wake_id);
        reopened
            .transition_agent_wake_at(
                &retry.wake_id,
                AgentWakeStatus::Claimed,
                AgentWakeStatus::Cancelled,
                Some(&retry_claim),
                recovered_at + 2,
            )
            .unwrap();
        maximum_restart_micros = maximum_restart_micros.max(started.elapsed().as_micros());
    }

    let query_started = Instant::now();
    let connection = fixture.service.state.connection().unwrap();
    let ordinary_messages = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'message'",
            [],
            |row| row.get::<_, usize>(0),
        )
        .unwrap();
    let result_messages = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages WHERE kind = 'result'",
            [],
            |row| row.get::<_, usize>(0),
        )
        .unwrap();
    let (event_count, maximum_event_sequence) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(root_sequence), 0)
                   FROM agent_collaboration_events
                  WHERE root_agent_id = 'agent-root'",
            [],
            |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?)),
        )
        .unwrap();
    let ghost_wakes = connection
        .query_row(
            "SELECT COUNT(*) FROM agent_wake_requests
                  WHERE status IN ('claimed', 'running', 'waiting_for_approval')",
            [],
            |row| row.get::<_, usize>(0),
        )
        .unwrap();
    let query_micros = query_started.elapsed().as_micros();
    drop(connection);

    assert_eq!(ordinary_messages, fact_count);
    assert_eq!(result_messages, CONFIGURED_TREE_NODES - 1);
    assert!(
        event_count >= fact_count,
        "every message commit invalidates the tree"
    );
    assert_eq!(
        event_count, maximum_event_sequence,
        "root sequence has no gap"
    );
    assert_eq!(ghost_wakes, 0);

    let event_catchup_started = Instant::now();
    let mut event_cursor = 0_u64;
    let mut observed_events = 0_usize;
    let mut maximum_event_page_micros = 0_u128;
    loop {
        let page_started = Instant::now();
        let page = fixture
            .service
            .list_agent_collaboration_events("agent-root", event_cursor, 512)
            .unwrap();
        maximum_event_page_micros =
            maximum_event_page_micros.max(page_started.elapsed().as_micros());
        if page.is_empty() {
            break;
        }
        for event in &page {
            event_cursor += 1;
            assert_eq!(event.root_sequence, event_cursor, "catch-up sequence gap");
        }
        observed_events += page.len();
    }
    let event_catchup_micros = event_catchup_started.elapsed().as_micros();
    assert_eq!(observed_events, event_count);

    let reopened = StorageService::open(&database_path).unwrap();
    assert_eq!(reopened.list_agent_tree("agent-root").unwrap().len(), 64);
    assert!(
        reopened
            .latest_agent_collaboration_event_sequence("agent-root")
            .unwrap()
            >= fact_count as u64
    );

    let database_bytes = std::fs::metadata(&database_path).unwrap().len();
    eprintln!(
            "ROUND6_METRIC tree_nodes=64 facts={fact_count} results={result_messages} events={event_count} writers={WRITERS} sqlite_busy_errors=0 max_enqueue_us={} query_us={query_micros} event_catchup_us={event_catchup_micros} max_event_page_us={maximum_event_page_micros} restarts={restart_count} max_restart_us={maximum_restart_micros} database_bytes={database_bytes} wall_ms={}",
            maximum_enqueue_micros.load(Ordering::Relaxed),
            profile_started.elapsed().as_millis(),
        );
}
