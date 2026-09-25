use super::*;
use crate::storage::{migrations::run_migrations, service::StorageService};
use crate::workflow::{
    BoundaryPoint, BoundaryPositions, Endpoint, Flow, Mode, Node, Rule, Viewport,
};

fn graph() -> Definition {
    let rule = Rule {
        mode: Mode::All,
        min: 0,
        max: 0,
        required: vec![],
        groups: vec![],
    };
    Definition {
        boundary_positions: BoundaryPositions {
            input: BoundaryPoint {
                x: -180.0,
                y: 200.0,
            },
            output: BoundaryPoint { x: 564.0, y: 200.0 },
        },
        schema_version: 1,
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
            template_id: None,
            model_config_id: Some("model-review".into()),
            receives: "Code change".into(),
            task: "Review actual defects".into(),
            delivers: "Findings".into(),
            input_rule: rule.clone(),
            output_rule: rule,
            x: 100.0,
            y: 200.0,
        }],
        flows: vec![
            Flow {
                id: "input".into(),
                name: "Task".into(),
                source: Endpoint::Boundary,
                target: Endpoint::Node {
                    node_id: "review".into(),
                },
            },
            Flow {
                id: "output".into(),
                name: "Findings".into(),
                source: Endpoint::Node {
                    node_id: "review".into(),
                },
                target: Endpoint::Boundary,
            },
        ],
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
fn legacy_v1_storage_is_read_without_mutation_and_can_be_saved_after_selecting_a_model() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut json = serde_json::to_value(graph()).unwrap();
    json["nodes"][0]
        .as_object_mut()
        .unwrap()
        .remove("modelConfigId");
    json.as_object_mut().unwrap().remove("boundaryPositions");
    let legacy = json.to_string();
    connection.execute("INSERT INTO workflow_definitions(workflow_id,definition_json,revision,updated_at) VALUES (?1,?2,1,1)", params![graph().id, legacy]).unwrap();
    let mut record = request(&mut connection, Request::List)
        .unwrap()
        .records
        .remove(0);
    assert!(record.definition.nodes[0].model_config_id.is_none());
    assert_eq!(
        record.definition.boundary_positions,
        graph().boundary_positions
    );
    assert_eq!(record.issues[0].code, "node_model");
    assert_eq!(
        connection
            .query_row(
                "SELECT definition_json FROM workflow_definitions",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        legacy
    );
    record.definition.nodes[0].model_config_id = Some("model-review".into());
    record.definition.boundary_positions.input = BoundaryPoint { x: -60.5, y: 320.0 };
    let saved = save(&mut connection, record.definition, 1).unwrap();
    assert!(saved.records[0].issues.is_empty());
    assert_eq!(saved.records[0].revision, 2);
    assert_eq!(
        saved.records[0].definition.boundary_positions.input,
        BoundaryPoint { x: -60.5, y: 320.0 }
    );
    assert_eq!(
        request(&mut connection, Request::List).unwrap().records[0]
            .definition
            .nodes[0]
            .model_config_id
            .as_deref(),
        Some("model-review")
    );
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
    service
        .save_model_settings(settings(false, "owned-test-token"))
        .unwrap();
    let disabled = service.workflow_request(Request::List).unwrap();
    assert_eq!(disabled.records[0].issues[0].code, "node_model_unavailable");
    service.save_model_settings(settings(true, "")).unwrap();
    let no_credential = service.workflow_request(Request::List).unwrap();
    assert_eq!(
        no_credential.records[0].issues[0].code,
        "node_model_unavailable"
    );
    assert_eq!(no_credential.records[0].revision, 1);
    assert_eq!(
        no_credential.records[0].definition.nodes[0]
            .model_config_id
            .as_deref(),
        Some("model-review")
    );
    service
        .save_model_settings(settings(true, "owned-test-token"))
        .unwrap();
    assert!(service.workflow_request(Request::List).unwrap().records[0]
        .issues
        .is_empty());
}

#[test]
fn template_model_overrides_cannot_replace_a_saved_definition() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    let mut invalid = graph();
    invalid.nodes[0].template_id = Some("template-review".into());
    assert!(matches!(
        save(&mut connection, invalid, 1),
        Err(Error::Invalid(_))
    ));
    let read = request(&mut connection, Request::List).unwrap();
    assert_eq!(read.records[0].revision, 1);
    assert!(read.records[0].definition.nodes[0].template_id.is_none());
}

#[test]
fn invalid_boundary_positions_cannot_replace_persisted_layout() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    save(&mut connection, graph(), 0).unwrap();
    let mut invalid = graph();
    invalid.boundary_positions.output.y = 100001.0;
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
fn missing_template_is_a_draft_issue_recomputed_on_list() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut definition = graph();
    definition.nodes[0].template_id = Some("template-review".into());
    definition.nodes[0].model_config_id = None;
    let response = save(&mut connection, definition, 0).unwrap();
    assert_eq!(response.records[0].issues[0].code, "template");
    connection.execute_batch("INSERT INTO agent_templates (
        template_id,schema_version,machine_key,name,description,instructions,model_config_id,enabled,revision,created_at,updated_at
        ) VALUES ('template-review',1,'review','Review','','Review','model',1,1,1,1)").unwrap();
    assert!(request(&mut connection, Request::List).unwrap().records[0]
        .issues
        .is_empty());
    connection
        .execute(
            "DELETE FROM agent_templates WHERE template_id='template-review'",
            [],
        )
        .unwrap();
    let read = request(&mut connection, Request::List).unwrap();
    assert_eq!(read.records[0].issues[0].code, "template");
    assert_eq!(read.records[0].revision, 1);
}

#[test]
fn semantic_drafts_save_but_structural_corruption_is_rejected_without_mutation() {
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let mut definition = graph();
    definition.nodes[0].task.clear();
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
    large.nodes[0].task = "x".repeat(126_000);
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
