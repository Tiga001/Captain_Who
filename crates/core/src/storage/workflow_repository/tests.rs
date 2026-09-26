use super::*;
use crate::storage::{migrations::run_migrations, service::StorageService};
use crate::workflow::{
    AgentConfig, BoundaryPoint, BoundaryPositions, Endpoint, Flow, Node, NodeConfig, Viewport,
};

fn graph() -> Definition {
    Definition {
        boundary_positions: BoundaryPositions {
            input: BoundaryPoint {
                x: -180.0,
                y: 200.0,
            },
        },
        schema_version: 1,
        next_flow_sequence: None,
        id: "workflow-review".into(),
        name: "Review".into(),
        description: "Review a change".into(),
        background: "Reusable review process".into(),
        viewport: Viewport {
            x: 42.0,
            y: 12.0,
            zoom: 1.25,
        },
        nodes: vec![Node {
            id: "review".into(),
            name: "Review".into(),
            config: NodeConfig::Agent(AgentConfig {
                permission_mode: crate::workflow::WorkflowPermissionMode::Default,
                model_config_id: Some("model-review".into()),
                receives: "Code change".into(),
                task: "Review actual defects".into(),
                delivers: "Findings".into(),
            }),
            x: 100.0,
            y: 200.0,
        }],
        flows: vec![Flow {
            source_anchor: None,
            target_anchor: None,
            id: "input".into(),
            name: "Task".into(),
            source: Endpoint::Boundary,
            target: Endpoint::Node {
                node_id: "review".into(),
            },
        }],
    }
}

fn request(connection: &mut Connection, request: Request) -> Result<Response, Error> {
    super::request(connection, request, &HashSet::from(["model-review".into()]))
}

fn save(
    connection: &mut Connection,
    definition: Definition,
    expected_revision: u64,
) -> Result<Response, Error> {
    request(
        connection,
        Request::Save {
            definition,
            expected_revision,
        },
    )
}

fn set_enabled(
    connection: &mut Connection,
    enabled: bool,
    expected_revision: u64,
) -> Result<Response, Error> {
    request(
        connection,
        Request::SetEnabled {
            id: graph().id,
            enabled,
            expected_revision,
        },
    )
}

#[test]
fn enabled_workflow_reopens_and_valid_edits_preserve_availability() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("enabled-workflow.sqlite");
    let mut connection = Connection::open(&path).unwrap();
    run_migrations(&connection).unwrap();
    let mut positioned = graph();
    positioned.next_flow_sequence = Some(24);
    positioned.flows[0].source_anchor = Some(crate::workflow::Anchor {
        side: crate::workflow::AnchorSide::Top,
        offset: 0.3,
    });
    positioned.flows[0].target_anchor = Some(crate::workflow::Anchor {
        side: crate::workflow::AnchorSide::Bottom,
        offset: 0.7,
    });
    let saved = save(&mut connection, positioned, 0).unwrap();
    assert!(saved.records[0].issues.is_empty());
    assert!(!saved.records[0].enabled);
    assert_eq!(saved.records[0].definition.next_flow_sequence, Some(24));
    let enabled = set_enabled(&mut connection, true, 1).unwrap();
    assert!(enabled.records[0].enabled);
    assert_eq!(enabled.records[0].revision, 2);
    assert!(enabled.records[0].updated_at > saved.records[0].updated_at);
    drop(connection);
    let mut connection = Connection::open(&path).unwrap();
    run_migrations(&connection).unwrap();
    assert_eq!(
        serde_json::to_value(&enabled).unwrap(),
        serde_json::to_value(request(&mut connection, Request::List).unwrap()).unwrap()
    );
    let edited = save(&mut connection, graph(), 2).unwrap();
    assert!(edited.records[0].enabled);
    assert_eq!(edited.records[0].revision, 3);
    let disabled = set_enabled(&mut connection, false, 3).unwrap();
    assert!(!disabled.records[0].enabled);
    assert_eq!(disabled.records[0].revision, 4);
}

#[test]
fn saving_incomplete_draft_disables_it_and_valid_repair_does_not_reenable() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    set_enabled(&mut connection, true, 1).unwrap();
    let mut incomplete = graph();
    incomplete.nodes[0].agent_mut().task.clear();
    let draft = save(&mut connection, incomplete, 2).unwrap();
    assert!(!draft.records[0].enabled);
    assert_eq!(draft.records[0].revision, 3);
    assert!(draft.records[0]
        .issues
        .iter()
        .any(|issue| issue.code == "task"));
    for enabled in [true, false] {
        assert!(matches!(
            set_enabled(&mut connection, enabled, 3),
            Err(Error::Invalid(_))
        ));
    }
    let repaired = save(&mut connection, graph(), 3).unwrap();
    assert!(repaired.records[0].issues.is_empty());
    assert!(!repaired.records[0].enabled);
    assert_eq!(repaired.records[0].revision, 4);
}

#[test]
fn list_is_fail_closed_without_writing_and_toggle_revalidates_current_dependencies() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    set_enabled(&mut connection, true, 1).unwrap();
    let unavailable = super::request(&mut connection, Request::List, &HashSet::new()).unwrap();
    assert!(!unavailable.records[0].enabled);
    assert_eq!(
        unavailable.records[0].issues[0].code,
        "node_model_unavailable"
    );
    assert_eq!(unavailable.records[0].revision, 2);
    for enabled in [true, false] {
        assert!(matches!(
            super::request(
                &mut connection,
                Request::SetEnabled {
                    id: graph().id,
                    enabled,
                    expected_revision: 2,
                },
                &HashSet::new()
            ),
            Err(Error::Invalid(_))
        ));
    }
    assert!(connection
        .query_row("SELECT enabled FROM workflow_definitions", [], |row| row
            .get::<_, bool>(
            0
        ))
        .unwrap());
    let restored = request(&mut connection, Request::List).unwrap();
    assert!(restored.records[0].enabled);
    assert_eq!(restored.records[0].revision, 2);
    let persisted_invalid = super::request(
        &mut connection,
        Request::Save {
            definition: graph(),
            expected_revision: 2,
        },
        &HashSet::new(),
    )
    .unwrap();
    assert!(!persisted_invalid.records[0].enabled);
    assert!(!request(&mut connection, Request::List).unwrap().records[0].enabled);
}

#[test]
fn availability_changes_require_current_saved_revision_and_preserve_conflicting_state() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    assert!(matches!(
        set_enabled(&mut connection, true, 0),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        set_enabled(&mut connection, true, 1),
        Err(Error::Conflict(_))
    ));
    save(&mut connection, graph(), 0).unwrap();
    set_enabled(&mut connection, true, 1).unwrap();
    assert!(matches!(
        set_enabled(&mut connection, false, 1),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        save(&mut connection, graph(), 1),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        request(
            &mut connection,
            Request::Delete {
                id: graph().id,
                expected_revision: 1,
            }
        ),
        Err(Error::Conflict(_))
    ));
    assert!(request(&mut connection, Request::List).unwrap().records[0].enabled);
    connection
        .execute(
            "UPDATE workflow_definitions SET revision=?1",
            [MAX_SAFE_REVISION],
        )
        .unwrap();
    for expected in [MAX_SAFE_REVISION as u64, MAX_SAFE_REVISION as u64 + 1] {
        assert!(matches!(
            set_enabled(&mut connection, false, expected),
            Err(Error::Invalid(_))
        ));
    }
    let current = request(&mut connection, Request::List).unwrap();
    assert!(current.records[0].enabled);
    assert_eq!(current.records[0].revision, MAX_SAFE_REVISION as u64);
}

#[test]
fn workflow_reopens_with_exact_graph_layout_and_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workflows.sqlite");
    let service = StorageService::open(&path).unwrap();
    let first = service
        .workflow_request(Request::Save {
            definition: graph(),
            expected_revision: 0,
        })
        .unwrap();
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.records[0].issues[0].code, "node_model_unavailable");
    assert_eq!(first.records[0].revision, 1);
    drop(service);
    let service = StorageService::open(&path).unwrap();
    let read = service.workflow_request(Request::List).unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&read).unwrap()
    );
    assert_eq!(read.records[0].definition.nodes[0].x, 100.0);
    assert_eq!(read.records[0].definition.viewport.zoom, 1.25);
}

#[test]
fn service_uses_execution_projection_for_disabled_and_credential_missing_models() {
    use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord};
    use crate::{ProviderProfileConfig, ProviderProtocolDialect};
    let directory = tempfile::tempdir().unwrap();
    let service = StorageService::open(&directory.path().join("models.sqlite")).unwrap();
    let settings = |enabled: bool, token: &str| ModelSettingsRecord {
        api_url: "https://provider.example/v1/chat/completions".into(),
        api_token: token.into(),
        search_mode: "disabled".into(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-review".into(),
            provider_model_id: "review-model".into(),
            display_name: "Review model".into(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(64_000),
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".into(),
            cached_input_price: String::new(),
            output_price: "0".into(),
            enabled,
        }],
    };
    service
        .save_model_settings(settings(true, "owned-test-token"))
        .unwrap();
    let saved = service
        .workflow_request(Request::Save {
            definition: graph(),
            expected_revision: 0,
        })
        .unwrap();
    assert!(saved.records[0].issues.is_empty());
    let enabled_workflow = service
        .workflow_request(Request::SetEnabled {
            id: graph().id,
            enabled: true,
            expected_revision: 1,
        })
        .unwrap();
    assert!(enabled_workflow.records[0].enabled);
    service
        .save_model_settings(settings(false, "owned-test-token"))
        .unwrap();
    let disabled = service.workflow_request(Request::List).unwrap();
    assert!(!disabled.records[0].enabled);
    assert_eq!(disabled.records[0].issues[0].code, "node_model_unavailable");
    service.save_model_settings(settings(true, "")).unwrap();
    let mut no_credential = service.workflow_request(Request::List).unwrap();
    assert_eq!(
        no_credential.records[0].issues[0].code,
        "node_model_unavailable"
    );
    assert!(!no_credential.records[0].enabled);
    assert_eq!(no_credential.records[0].revision, 2);
    assert_eq!(
        no_credential.records[0].definition.nodes[0]
            .agent_mut()
            .model_config_id
            .as_deref(),
        Some("model-review")
    );
    service
        .save_model_settings(settings(true, "owned-test-token"))
        .unwrap();
    let restored = service.workflow_request(Request::List).unwrap();
    assert!(restored.records[0].issues.is_empty());
    assert!(restored.records[0].enabled);
}

#[test]
fn permission_modes_round_trip_without_creating_conversations() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    for (index, mode) in [
        crate::workflow::WorkflowPermissionMode::Default,
        crate::workflow::WorkflowPermissionMode::Custom,
        crate::workflow::WorkflowPermissionMode::Full,
    ]
    .into_iter()
    .enumerate()
    {
        let mut definition = graph();
        definition.nodes[0].agent_mut().permission_mode = mode.clone();
        save(&mut connection, definition, index as u64).unwrap();
        let read = request(&mut connection, Request::List).unwrap();
        assert!(read.records[0].issues.is_empty());
        assert!(matches!(&read.records[0].definition.nodes[0].config,
            NodeConfig::Agent(agent) if agent.permission_mode == mode));
    }
}

#[test]
fn invalid_boundary_positions_cannot_replace_persisted_layout() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    let mut invalid = graph();
    invalid.boundary_positions.input.y = 100001.0;
    assert!(matches!(
        save(&mut connection, invalid, 1),
        Err(Error::Invalid(_))
    ));
    let read = request(&mut connection, Request::List).unwrap();
    assert_eq!(read.records[0].revision, 1);
    assert_eq!(
        read.records[0].definition.boundary_positions,
        graph().boundary_positions
    );
}

#[test]
fn stale_save_and_delete_never_change_current_workflow() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    let mut updated = graph();
    updated.name = "Updated".into();
    assert_eq!(
        save(&mut connection, updated, 1).unwrap().records[0].revision,
        2
    );
    assert!(matches!(
        save(&mut connection, graph(), 1),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        save(&mut connection, graph(), 0),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        request(
            &mut connection,
            Request::Delete {
                id: graph().id,
                expected_revision: 1
            }
        ),
        Err(Error::Conflict(_))
    ));
    let current = request(&mut connection, Request::List).unwrap();
    assert_eq!(current.records[0].definition.name, "Updated");
    assert_eq!(current.records[0].revision, 2);
    assert!(request(
        &mut connection,
        Request::Delete {
            id: graph().id,
            expected_revision: 2
        }
    )
    .unwrap()
    .records
    .is_empty());
    assert!(matches!(
        save(&mut connection, graph(), 2),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn workflow_catalog_does_not_depend_on_subagent_templates() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    connection
        .execute("DROP TABLE agent_templates", [])
        .unwrap();
    let read = request(&mut connection, Request::List).unwrap();
    assert!(read.records[0].issues.is_empty());
    assert_eq!(read.records[0].revision, 1);
}

#[test]
fn semantic_drafts_save_but_structural_corruption_is_rejected_without_mutation() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut definition = graph();
    definition.nodes[0].agent_mut().task.clear();
    definition.flows.clear();
    let validation = request(
        &mut connection,
        Request::Validate {
            definition: definition.clone(),
        },
    )
    .unwrap();
    assert!(validation.records.is_empty());
    assert!(validation.issues.iter().any(|issue| issue.code == "entry"));
    let saved = save(&mut connection, definition.clone(), 0).unwrap();
    assert_eq!(saved.records[0].issues, validation.issues);
    definition.schema_version = 2;
    assert!(matches!(
        save(&mut connection, definition, 1),
        Err(Error::Invalid(_))
    ));
    let mut definition = graph();
    definition.flows[0].source = definition.flows[0].target.clone();
    assert!(matches!(
        save(&mut connection, definition, 1),
        Err(Error::Invalid(_))
    ));
    let mut definition = graph();
    definition.flows[0].target = Endpoint::Node {
        node_id: "missing".into(),
    };
    assert!(matches!(
        save(&mut connection, definition, 1),
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        request(&mut connection, Request::List).unwrap().records[0].revision,
        1
    );
}

#[test]
fn revisions_remain_exact_in_javascript_and_zero_cannot_delete() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    assert!(matches!(
        request(
            &mut connection,
            Request::Delete {
                id: graph().id,
                expected_revision: 0
            }
        ),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        save(&mut connection, graph(), MAX_SAFE_REVISION as u64 + 1),
        Err(Error::Invalid(_))
    ));
    connection
        .execute(
            "UPDATE workflow_definitions SET revision = ?1",
            [MAX_SAFE_REVISION],
        )
        .unwrap();
    assert!(matches!(
        save(&mut connection, graph(), MAX_SAFE_REVISION as u64),
        Err(Error::Invalid(_))
    ));
    assert_eq!(
        request(&mut connection, Request::List).unwrap().records[0].revision,
        MAX_SAFE_REVISION as u64
    );
    assert!(request(
        &mut connection,
        Request::Delete {
            id: graph().id,
            expected_revision: MAX_SAFE_REVISION as u64
        }
    )
    .unwrap()
    .records
    .is_empty());
}

#[test]
fn definition_capacity_rejects_creation_but_allows_editing_and_deleting() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..MAX_DEFINITIONS {
        let mut definition = graph();
        definition.id = format!("workflow-{index}");
        transaction.execute(
            "INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at) VALUES (?1,?2,1,1)",
            params![definition.id, serde_json::to_string(&definition).unwrap()],
        ).unwrap();
    }
    transaction.commit().unwrap();
    assert!(matches!(
        save(&mut connection, graph(), 0),
        Err(Error::Invalid(_))
    ));
    let mut definition = graph();
    definition.id = "workflow-0".into();
    definition.name = "Edited".into();
    let updated = save(&mut connection, definition, 1).unwrap();
    assert_eq!(updated.records.len(), MAX_DEFINITIONS);
    assert_eq!(updated.records[0].definition.name, "Edited");
    let deleted = request(
        &mut connection,
        Request::Delete {
            id: "workflow-1".into(),
            expected_revision: 1,
        },
    )
    .unwrap();
    assert_eq!(deleted.records.len(), MAX_DEFINITIONS - 1);
    assert!(save(&mut connection, graph(), 0).is_ok());
}

#[test]
fn catalog_byte_quota_accounts_for_replacements_and_bounds_reads() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut large = graph();
    large.nodes[0].agent_mut().task = "x".repeat(126_000);
    for index in 1..10 {
        let mut node = large.nodes[0].clone();
        node.id = format!("extra-{index}");
        large.nodes.push(node);
    }
    let transaction = connection.transaction().unwrap();
    for index in 0..13 {
        large.id = format!("large-{index}");
        transaction.execute(
            "INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at) VALUES (?1,?2,1,1)",
            params![large.id, serde_json::to_string(&large).unwrap()],
        ).unwrap();
    }
    transaction.commit().unwrap();
    assert!(catalog_size(&connection).unwrap().1 < MAX_CATALOG_BYTES);
    large.id = "additional".into();
    assert!(matches!(
        save(&mut connection, large.clone(), 0),
        Err(Error::Invalid(_))
    ));
    let mut larger = large.clone();
    larger.id = "large-0".into();
    for index in 10..15 {
        let mut node = larger.nodes[0].clone();
        node.id = format!("extra-{index}");
        larger.nodes.push(node);
    }
    assert!(matches!(
        save(&mut connection, larger, 1),
        Err(Error::Invalid(_))
    ));
    let mut smaller = graph();
    smaller.id = "large-0".into();
    save(&mut connection, smaller, 1).unwrap();
    assert!(save(&mut connection, large.clone(), 0).is_ok());
    large.id = "externally-corrupted-catalog".into();
    connection.execute(
        "INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at) VALUES (?1,?2,1,1)",
        params![large.id, serde_json::to_string(&large).unwrap()],
    ).unwrap();
    assert!(matches!(
        request(&mut connection, Request::List),
        Err(Error::Storage(_))
    ));
}

#[test]
fn independent_gates_reopen_with_modes_and_disconnected_input_saves_as_disabled_draft() {
    use crate::workflow::{BusyPolicy, InputProcessingMode};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("logic-gates.sqlite");
    let mut definition: Definition = serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/workflow-definition-v1.json"
    ))
    .unwrap();
    for node in &mut definition.nodes {
        match &mut node.config {
            NodeConfig::User { .. } => {}
            NodeConfig::Agent(agent) => agent.model_config_id = Some("model-review".into()),
            NodeConfig::InputGate {
                processing_mode,
                busy_policy,
            } => {
                *processing_mode = InputProcessingMode::Individual;
                *busy_policy = BusyPolicy::Inject;
            }
            NodeConfig::OutputGate { selection } => {
                selection.mode = crate::workflow::Mode::Exact;
                selection.min = 2;
            }
        }
    }
    let expected = serde_json::to_value(&definition).unwrap();
    {
        let mut connection = Connection::open(&path).unwrap();
        run_migrations(&connection).unwrap();
        let saved = save(&mut connection, definition.clone(), 0).unwrap();
        assert!(saved.records[0].issues.is_empty());
        let enabled = request(
            &mut connection,
            Request::SetEnabled {
                id: definition.id.clone(),
                enabled: true,
                expected_revision: 1,
            },
        )
        .unwrap();
        assert!(enabled.records[0].enabled);
    }
    let mut connection = Connection::open(&path).unwrap();
    run_migrations(&connection).unwrap();
    let reopened = request(&mut connection, Request::List).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.records[0].definition).unwrap(),
        expected
    );
    assert!(reopened.records[0].enabled);
    definition
        .flows
        .retain(|flow| flow.target.node() != Some("implement-input"));
    let saved = save(&mut connection, definition.clone(), 2).unwrap();
    assert!(!saved.records[0].enabled);
    assert!(saved.records[0]
        .issues
        .iter()
        .any(|i| i.code == "inputRule"));
    assert!(matches!(
        request(
            &mut connection,
            Request::SetEnabled {
                id: definition.id.clone(),
                enabled: true,
                expected_revision: 3
            }
        ),
        Err(Error::Invalid(_))
    ));
    let reopened = request(&mut connection, Request::List).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.records[0].definition).unwrap(),
        serde_json::to_value(&definition).unwrap()
    );
}

#[test]
fn visual_user_node_round_trips_without_agent_or_gate_configuration() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut definition = graph();
    definition.nodes[0].config = NodeConfig::User {
        task: "Review the proposal".into(),
    };
    definition.nodes[0].name = "User".into();
    let expected = serde_json::to_value(&definition).unwrap();
    save(&mut connection, definition, 0).unwrap();
    let reopened = request(&mut connection, Request::List).unwrap();
    assert_eq!(
        serde_json::to_value(&reopened.records[0].definition).unwrap(),
        expected
    );
    let mut invalid = expected;
    invalid["nodes"][0]["modelConfigId"] = serde_json::json!("Not an agent");
    assert!(serde_json::from_value::<Definition>(invalid).is_err());
}
