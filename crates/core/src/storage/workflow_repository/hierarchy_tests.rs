use super::*;
use crate::storage::{migrations::run_migrations, service::StorageService};
use crate::workflow::ManagementRole;
use serde_json::json;

fn definition() -> Definition {
    serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/organization-hierarchy-v1.json"
    ))
    .unwrap()
}

#[test]
fn organization_hierarchy_template_draft_and_duplicate_survive_storage_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("organization.sqlite");
    let service = StorageService::open(&path).unwrap();
    let saved = service
        .workflow_request(Request::Save {
            definition: definition(),
            expected_revision: 0,
        })
        .unwrap();
    let expected = serde_json::to_value(&saved.records[0].definition).unwrap();
    let mut draft = definition();
    draft.nodes[1].rank = 7;
    draft.departments[1].name = "Senior review team".into();
    service
        .workflow_request(
            serde_json::from_value(json!({
                "operation":"saveDraft", "definition":draft,
                "expectedRevision":1, "expectedDraftRevision":0
            }))
            .unwrap(),
        )
        .unwrap();
    service
        .workflow_request(
            serde_json::from_value(json!({
                "operation":"duplicate", "id":"organization-hierarchy", "name":"Copy",
                "newId":"organization-copy", "expectedRevision":1
            }))
            .unwrap(),
        )
        .unwrap();
    drop(service);

    let service = StorageService::open(&path).unwrap();
    let read = service.workflow_request(Request::List).unwrap();
    let record = read
        .records
        .iter()
        .find(|r| r.definition.id == "organization-hierarchy")
        .unwrap();
    assert_eq!(serde_json::to_value(&record.definition).unwrap(), expected);
    let copy = read
        .records
        .iter()
        .find(|r| r.definition.id == "organization-copy")
        .unwrap();
    assert_eq!(copy.definition.nodes[1].rank, 5);
    assert_eq!(
        copy.definition.nodes[1].management_role,
        ManagementRole::DepartmentAdmin
    );
    assert_eq!(
        copy.definition.nodes[1].department_id.as_deref(),
        Some("review-team")
    );
    assert_eq!(
        copy.definition.departments[1].parent_id.as_deref(),
        Some("research")
    );
    assert_eq!(read.drafts[0].definition.nodes[1].rank, 7);
    assert_eq!(
        read.drafts[0].definition.departments[1].name,
        "Senior review team"
    );
}

#[test]
fn organization_hierarchy_rejects_invalid_identity_tree_geometry_and_rank() {
    let original = definition();
    let mut malformed = vec![];
    let mut invalid = original.clone();
    invalid.departments[0].parent_id = Some("review-team".into());
    malformed.push(invalid);
    let mut invalid = original.clone();
    invalid.departments[1].parent_id = Some("missing".into());
    malformed.push(invalid);
    let mut invalid = original.clone();
    invalid.departments.push(invalid.departments[0].clone());
    malformed.push(invalid);
    let mut invalid = original.clone();
    invalid.departments[0].id = invalid.nodes[0].id.clone();
    malformed.push(invalid);
    let mut invalid = original.clone();
    invalid.nodes[0].department_id = Some("missing".into());
    malformed.push(invalid);
    for rank in [0, 100] {
        let mut invalid = original.clone();
        invalid.nodes[0].rank = rank;
        malformed.push(invalid);
    }
    for width in [0.0, -1.0, 100_001.0, f64::INFINITY] {
        let mut invalid = original.clone();
        invalid.departments[0].width = width;
        malformed.push(invalid);
    }
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    for invalid in malformed {
        assert!(matches!(
            super::request(
                &mut connection,
                Request::Save {
                    definition: invalid,
                    expected_revision: 0,
                },
                &HashSet::new()
            ),
            Err(Error::Invalid(_))
        ));
    }
    assert!(
        super::request(&mut connection, Request::List, &HashSet::new())
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn organization_hierarchy_incomplete_scope_is_a_readiness_issue_without_erasing_the_draft() {
    let mut draft = definition();
    draft.departments[0].name.clear();
    draft.nodes[1].department_id = None;
    let issues = draft.validate(&HashSet::new()).unwrap();
    assert!(issues
        .iter()
        .any(|issue| issue.code == "department_name" && issue.subject == "research"));
    assert!(issues
        .iter()
        .any(|issue| issue.code == "department_admin_scope" && issue.subject == "review"));
    let mut connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    let response = super::request(
        &mut connection,
        Request::Save {
            definition: draft,
            expected_revision: 0,
        },
        &HashSet::new(),
    )
    .unwrap();
    assert!(!response.records[0].enabled);
    assert_eq!(
        response.records[0].definition.nodes[1].management_role,
        ManagementRole::DepartmentAdmin
    );
}

#[test]
fn organization_member_names_share_unicode_comparison_with_renderer() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../../packages/protocol/fixtures/organization-member-names-v1.json"
    ))
    .unwrap();
    for case in cases {
        assert_eq!(
            crate::workflow::member_name_key(case["name"].as_str().unwrap()),
            case["key"].as_str().unwrap()
        );
    }
}

#[test]
fn organization_member_names_reject_writes_across_departments_but_keep_old_data_editable() {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    let models = HashSet::new();
    let original = definition();
    super::request(
        &mut c,
        Request::Save {
            definition: original.clone(),
            expected_revision: 0,
        },
        &models,
    )
    .unwrap();
    let mut duplicate = original.clone();
    duplicate.nodes[0].name = "Boss".into();
    duplicate.nodes[1].name = "　bOSS ".into();
    let issues = duplicate.validate(&models).unwrap();
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.code == "node_name_duplicate")
            .count(),
        2
    );
    for request in [
        json!({"operation":"save","definition":duplicate,"expectedRevision":1}),
        json!({"operation":"saveDraft","definition":duplicate,"expectedRevision":1,"expectedDraftRevision":0}),
        json!({"operation":"saveInstance","id":"instance","definition":duplicate,"name":"Team","color":"#123456","expectedRevision":0,"bindings":[]}),
    ] {
        let error =
            super::request(&mut c, serde_json::from_value(request).unwrap(), &models).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("organization_duplicate_member_name"),
            "{error}"
        );
    }
    let loaded = super::request(&mut c, Request::List, &models).unwrap();
    assert_eq!(loaded.records[0].revision, 1);
    assert!(loaded.drafts.is_empty());
    assert!(loaded.instances.is_empty());
    // A pre-upgrade record with repeated names can still open and be corrected.
    c.execute(
        "UPDATE workflow_definitions SET definition_json=?1",
        [serde_json::to_string(&duplicate).unwrap()],
    )
    .unwrap();
    let loaded = super::request(&mut c, Request::List, &models).unwrap();
    assert_eq!(loaded.records.len(), 1);
    assert!(!loaded.records[0].enabled);
    assert!(loaded.invalid_records.is_empty());
    super::request(
        &mut c,
        Request::Save {
            definition: original,
            expected_revision: 1,
        },
        &models,
    )
    .unwrap();
    duplicate.nodes[0].name.clear();
    duplicate.nodes[1].name = "　 ".into();
    assert!(duplicate.validate_unique_member_names().is_ok());
    assert_eq!(
        duplicate
            .validate(&models)
            .unwrap()
            .iter()
            .filter(|i| i.code == "task")
            .count(),
        2
    );
}

#[test]
fn organization_department_paths_reject_ambiguous_writes_and_allow_distinct_parent_scopes() {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    let models = HashSet::new();
    let original = definition();
    super::request(
        &mut c,
        Request::Save {
            definition: original.clone(),
            expected_revision: 0,
        },
        &models,
    )
    .unwrap();
    for (name, duplicate, code) in [
        (
            "　RESEARCH ",
            true,
            "organization_duplicate_department_name",
        ),
        (
            "Human Resources/Payroll",
            false,
            "organization_department_name_separator",
        ),
    ] {
        let mut invalid = original.clone();
        invalid.departments[0].name = "Research".into();
        invalid.departments[1].name = name.into();
        if duplicate {
            invalid.departments[1].parent_id = None;
        }
        assert!(invalid
            .validate_department_names()
            .unwrap_err()
            .contains(code));
        let issues = invalid.validate(&models).unwrap();
        assert!(issues.iter().any(|issue| issue.code
            == if duplicate {
                "department_name_duplicate"
            } else {
                "department_name_separator"
            }));
        for request in [
            json!({"operation":"save","definition":invalid,"expectedRevision":1}),
            json!({"operation":"saveDraft","definition":invalid,"expectedRevision":1,"expectedDraftRevision":0}),
            json!({"operation":"saveInstance","id":"instance","definition":invalid,"name":"Team","color":"#123456","expectedRevision":0,"bindings":[]}),
        ] {
            let error = super::request(&mut c, serde_json::from_value(request).unwrap(), &models)
                .unwrap_err();
            assert!(error.to_string().contains(code), "{error}");
        }
        // Personnel edits use the same write barrier before mutating the organization.
        assert!(save_instance_definition(&c, "instance", &invalid, 1)
            .unwrap_err()
            .to_string()
            .contains(code));
    }
    let loaded = super::request(&mut c, Request::List, &models).unwrap();
    assert_eq!(loaded.records[0].revision, 1);
    assert!(loaded.drafts.is_empty());
    assert!(loaded.instances.is_empty());
    let mut scoped = original;
    scoped.departments[0].name = "Research".into();
    scoped.departments[1].name = "research".into();
    assert!(scoped.validate_department_names().is_ok());
}
