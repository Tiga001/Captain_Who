use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[test]
fn skill_catalog_v4_serializes_explicit_source_and_trust_unions() {
    let response = SkillsListResponse {
        schema_version: SKILL_CATALOG_SCHEMA_VERSION,
        catalog_revision: "catalog-revision".to_string(),
        skills: vec![SkillDescriptorDto {
            id: "installed:user:0190b0f2-7c50-7cc0-8b25-3bb80f08b334".to_string(),
            name: "sample-skill".to_string(),
            description: "Exercise the installed Skill protocol fixture.".to_string(),
            source: SkillSourceDto {
                kind: SkillSourceKindDto::Installed,
                id: "installed:user".to_string(),
            },
            trust: SkillTrustDto::Untrusted,
            activation_scope: "run".to_string(),
            revision: concat!(
                "skill-package-sha256-v1:",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            )
            .to_string(),
            location: Some(
                concat!(
                    "packages/v1/",
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
                    "SKILL.md"
                )
                .to_string(),
            ),
        }],
        diagnostics: Vec::new(),
        truncated: false,
    };

    let value = serde_json::to_value(response).unwrap();

    assert_eq!(value["schemaVersion"], 4);
    assert_eq!(value["skills"][0]["source"]["kind"], "installed");
    assert_eq!(value["skills"][0]["source"]["id"], "installed:user");
    assert_eq!(value["skills"][0]["trust"], "untrusted");
}

#[test]
fn skill_catalog_v4_accepts_installed_and_rejects_unknown_enum_values() {
    assert_eq!(
        serde_json::from_str::<SkillSourceKindDto>("\"installed\"").unwrap(),
        SkillSourceKindDto::Installed
    );
    assert!(serde_json::from_str::<SkillSourceKindDto>("\"remote\"").is_err());
    assert!(serde_json::from_str::<SkillTrustDto>("\"userApproved\"").is_err());
}

#[test]
fn skill_mutation_requests_are_strict_camel_case_contracts() {
    assert_eq!(SKILLS_INSTALL_LOCAL_METHOD, "skills.installLocal");
    assert_eq!(SKILLS_UPDATE_LOCAL_METHOD, "skills.updateLocal");
    assert_eq!(SKILLS_UNINSTALL_METHOD, "skills.uninstall");

    let install = serde_json::from_value::<SkillsInstallLocalRequest>(serde_json::json!({
        "installationId": "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
        "directory": "/tmp/local-skill"
    }))
    .unwrap();
    assert_eq!(
        install.installation_id,
        "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
    );
    assert_eq!(install.directory, "/tmp/local-skill");

    let update = serde_json::from_value::<SkillsUpdateLocalRequest>(serde_json::json!({
        "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
        "expectedRevision": "skill-package-sha256-v1:old",
        "directory": "/tmp/local-skill"
    }))
    .unwrap();
    assert_eq!(
        update.skill_id,
        "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
    );
    assert_eq!(update.expected_revision, "skill-package-sha256-v1:old");

    let uninstall = serde_json::from_value::<SkillsUninstallRequest>(serde_json::json!({
        "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
        "expectedRevision": "skill-package-sha256-v1:current"
    }))
    .unwrap();
    assert_eq!(
        uninstall.skill_id,
        "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d"
    );

    assert!(
        serde_json::from_value::<SkillsInstallLocalRequest>(serde_json::json!({
            "installationId": "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
            "directory": "/tmp/local-skill",
            "storeRoot": "/tmp/attacker-controlled"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsUpdateLocalRequest>(serde_json::json!({
            "skillId": "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d",
            "directory": "/tmp/local-skill"
        }))
        .is_err()
    );
}

#[test]
fn skill_mutation_v1_serializes_all_idempotent_outcomes() {
    assert_eq!(SKILL_MUTATION_SCHEMA_VERSION, 1);
    let install_outcomes = [
        (SkillInstallMutationOutcomeDto::Installed, "installed"),
        (
            SkillInstallMutationOutcomeDto::AlreadyInstalled,
            "alreadyInstalled",
        ),
    ];

    for (outcome, expected) in install_outcomes {
        let response = SkillMutationResponse::install(
            "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            "skill-package-sha256-v1:current".to_string(),
            outcome,
        );
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);
        assert_eq!(value["outcome"], expected);
        assert_eq!(value["revision"], "skill-package-sha256-v1:current");
    }

    let update_outcomes = [
        (SkillUpdateMutationOutcomeDto::Updated, "updated"),
        (
            SkillUpdateMutationOutcomeDto::AlreadyCurrent,
            "alreadyCurrent",
        ),
    ];

    for (outcome, expected) in update_outcomes {
        let response = SkillMutationResponse::update(
            "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            "skill-package-sha256-v1:current".to_string(),
            outcome,
        );
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);
        assert_eq!(value["outcome"], expected);
        assert_eq!(value["revision"], "skill-package-sha256-v1:current");
    }

    let removal_outcomes = [
        (SkillRemovalMutationOutcomeDto::Uninstalled, "uninstalled"),
        (
            SkillRemovalMutationOutcomeDto::AlreadyAbsent,
            "alreadyAbsent",
        ),
    ];
    for (outcome, expected) in removal_outcomes {
        let response = SkillMutationResponse::removal(
            "018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            "installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string(),
            outcome,
        );
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["outcome"], expected);
        assert!(value.get("revision").is_none());
    }
}

#[test]
fn skill_mutation_v1_matches_the_shared_rust_typescript_wire_golden() {
    let golden: serde_json::Value = serde_json::from_str(include_str!(
        "../../../packages/protocol/fixtures/skill-mutation-v1.json"
    ))
    .unwrap();
    assert_eq!(golden["schemaVersion"], SKILL_MUTATION_SCHEMA_VERSION);

    for case in golden["cases"].as_array().unwrap() {
        let expected = &case["response"];
        let installation_id = expected["installationId"].as_str().unwrap().to_string();
        let skill_id = expected["skillId"].as_str().unwrap().to_string();
        let outcome = expected["outcome"].as_str().unwrap();
        let response = match (case["operation"].as_str().unwrap(), outcome) {
            ("install", "installed") => SkillMutationResponse::install(
                installation_id,
                skill_id,
                expected["revision"].as_str().unwrap().to_string(),
                SkillInstallMutationOutcomeDto::Installed,
            ),
            ("install", "alreadyInstalled") => SkillMutationResponse::install(
                installation_id,
                skill_id,
                expected["revision"].as_str().unwrap().to_string(),
                SkillInstallMutationOutcomeDto::AlreadyInstalled,
            ),
            ("update", "updated") => SkillMutationResponse::update(
                installation_id,
                skill_id,
                expected["revision"].as_str().unwrap().to_string(),
                SkillUpdateMutationOutcomeDto::Updated,
            ),
            ("update", "alreadyCurrent") => SkillMutationResponse::update(
                installation_id,
                skill_id,
                expected["revision"].as_str().unwrap().to_string(),
                SkillUpdateMutationOutcomeDto::AlreadyCurrent,
            ),
            ("uninstall", "uninstalled") => SkillMutationResponse::removal(
                installation_id,
                skill_id,
                SkillRemovalMutationOutcomeDto::Uninstalled,
            ),
            ("uninstall", "alreadyAbsent") => SkillMutationResponse::removal(
                installation_id,
                skill_id,
                SkillRemovalMutationOutcomeDto::AlreadyAbsent,
            ),
            combination => panic!("unexpected shared Skill mutation case {combination:?}"),
        };

        assert_eq!(serde_json::to_value(response).unwrap(), *expected);
    }
}

#[test]
fn skill_installation_workflow_v1_matches_the_shared_wire_golden() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../packages/protocol/fixtures/skill-installation-workflow-v1.json"
    ))
    .unwrap();
    assert_eq!(
        golden["workflowSchemaVersion"],
        SKILL_INSTALLATION_WORKFLOW_SCHEMA_VERSION
    );
    assert_eq!(
        golden["managementSchemaVersion"],
        SKILL_MANAGEMENT_SCHEMA_VERSION
    );
    assert_eq!(SKILL_MANAGEMENT_ERROR_CODE, -32012);

    for case in golden["inspectCases"].as_array().unwrap() {
        assert_wire_round_trip::<SkillsInspectInstallationRequest>(&case["request"]);
        assert_wire_round_trip::<SkillInstallationPreviewDto>(&case["preview"]);
    }
    assert_wire_round_trip::<SkillsCommitInstallationRequest>(&golden["commit"]["request"]);
    assert_wire_round_trip::<SkillInstallationCommitResponse>(&golden["commit"]["response"]);
    assert_wire_round_trip::<SkillsCancelPreparationRequest>(&golden["cancel"]["request"]);
    assert_wire_round_trip::<SkillPreparationCancellationResponse>(&golden["cancel"]["response"]);
    assert_wire_round_trip::<SkillsListManagementRequest>(&golden["management"]["listRequest"]);
    assert_wire_round_trip::<SkillsListManagementResponse>(&golden["management"]["listResponse"]);
    assert_wire_round_trip::<SkillsSetEnabledRequest>(&golden["management"]["setEnabledRequest"]);
    assert_wire_round_trip::<SkillsSetEnabledResponse>(&golden["management"]["setEnabledResponse"]);
    assert_wire_round_trip::<SkillsChangedNotification>(&golden["management"]["changed"]);
    assert_wire_round_trip::<SkillInspectionErrorData>(&golden["inspectionError"]);
    for error in golden["inspectionErrors"].as_array().unwrap() {
        assert_wire_round_trip::<SkillInspectionErrorData>(error);
    }
    for error in golden["managementErrors"].as_array().unwrap() {
        assert_wire_round_trip::<SkillManagementErrorData>(error);
    }
}

#[test]
fn skill_source_resolution_v2_matches_the_shared_wire_golden() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../packages/protocol/fixtures/skill-source-resolution-v2.json"
    ))
    .unwrap();

    assert_eq!(
        golden["schemaVersion"],
        SKILL_SOURCE_RESOLUTION_SCHEMA_VERSION
    );
    assert_eq!(golden["method"], SKILLS_RESOLVE_INSTALLATION_SOURCE_METHOD);
    assert_eq!(
        golden["cancel"]["method"],
        SKILLS_CANCEL_SOURCE_RESOLUTION_METHOD
    );
    assert_eq!(SKILL_SOURCE_RESOLUTION_ERROR_CODE, -32013);
    for case in golden["cases"].as_array().unwrap() {
        assert_wire_round_trip::<SkillsResolveInstallationSourceRequest>(&case["request"]);
        assert_wire_round_trip::<SkillsResolveInstallationSourceResponse>(&case["response"]);
    }
    for error in golden["errors"].as_array().unwrap() {
        assert_wire_round_trip::<SkillSourceResolutionErrorData>(error);
    }
    for case in golden["cancel"]["cases"].as_array().unwrap() {
        assert_wire_round_trip::<SkillsCancelSourceResolutionRequest>(&case["request"]);
        assert_wire_round_trip::<SkillsCancelSourceResolutionResponse>(&case["response"]);
    }
}

#[test]
fn skill_source_resolution_boundaries_are_strict_and_immutable() {
    let golden: Value = serde_json::from_str(include_str!(
        "../../../packages/protocol/fixtures/skill-source-resolution-v2.json"
    ))
    .unwrap();

    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
            "resolutionId": "11111111-1111-4111-8111-111111111111",
            "locator": {
                "kind": "url",
                "url": "https://github.com/openai/example-skills",
                "credential": "must-not-cross-the-boundary"
            }
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsCancelSourceResolutionRequest>(serde_json::json!({
            "resolutionId": "11111111-1111-4111-8111-111111111111",
            "candidateId": "must-not-cross-the-boundary"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsCancelSourceResolutionResponse>(serde_json::json!({
            "schemaVersion": 2,
            "resolutionId": "11111111-1111-4111-8111-111111111111",
            "outcome": "forgotten"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
            "resolutionId": "00000000-0000-0000-0000-000000000000",
            "locator": {
                "kind": "url",
                "url": "https://github.com/openai/example-skills"
            }
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceRequest>(serde_json::json!({
            "resolutionId": "11111111-1111-4111-8111-111111111111",
            "locator": {
                "kind": "git",
                "url": "https://github.com/openai/example-skills"
            }
        }))
        .is_err()
    );

    let mut short_commit = golden["cases"][0]["response"].clone();
    short_commit["resolvedCommit"] = serde_json::json!("abc123");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(short_commit).is_err()
    );
    let mut uppercase_commit = golden["cases"][0]["response"].clone();
    uppercase_commit["resolvedCommit"] =
        serde_json::json!("ABCDEF0123456789ABCDEF0123456789ABCDEF01");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(uppercase_commit)
            .is_err()
    );

    let mut mismatched_candidate_commit = golden["cases"][0]["response"].clone();
    mismatched_candidate_commit["candidates"][0]["source"]["resolvedCommit"] =
        serde_json::json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
            mismatched_candidate_commit
        )
        .is_err()
    );

    let mut mismatched_tracking_commit = golden["cases"][0]["response"].clone();
    mismatched_tracking_commit["candidates"][0]["source"]["reference"] = serde_json::json!({
        "kind": "commit",
        "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    });
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(
            mismatched_tracking_commit
        )
        .is_err()
    );

    let mut missing_candidate_commit = golden["cases"][0]["response"].clone();
    missing_candidate_commit["candidates"][0]["source"]
        .as_object_mut()
        .unwrap()
        .remove("resolvedCommit");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(missing_candidate_commit)
            .is_err()
    );

    let mut mismatched_resolution_id = golden["cases"][0]["response"].clone();
    mismatched_resolution_id["candidates"][0]["acquisition"]["resolutionId"] =
        serde_json::json!("99999999-9999-4999-8999-999999999999");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(mismatched_resolution_id)
            .is_err()
    );

    let mut mismatched_candidate_id = golden["cases"][0]["response"].clone();
    mismatched_candidate_id["candidates"][0]["acquisition"]["candidateId"] =
        serde_json::json!("different-candidate");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(mismatched_candidate_id)
            .is_err()
    );

    for invalid_resolution_id in [
        "00000000-0000-0000-0000-000000000000",
        "11111111-1111-4111-8111-11111111111A",
        "not-a-uuid",
    ] {
        let mut response = golden["cases"][0]["response"].clone();
        response["resolutionId"] = serde_json::json!(invalid_resolution_id);
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(response).is_err()
        );
    }

    let mut invalid_expiry = golden["cases"][0]["response"].clone();
    invalid_expiry["expiresAtUnixMs"] = serde_json::json!(0);
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(invalid_expiry).is_err()
    );

    let mut invalid_cardinality = golden["cases"][1]["response"].clone();
    invalid_cardinality["outcome"] = serde_json::json!("resolved");
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(invalid_cardinality)
            .is_err()
    );
    let mut invalid_output: SkillsResolveInstallationSourceResponse =
        serde_json::from_value(golden["cases"][0]["response"].clone()).unwrap();
    invalid_output.outcome = SkillSourceResolutionOutcomeDto::SelectionRequired;
    assert!(serde_json::to_value(invalid_output).is_err());

    let mut duplicate_candidate_ids = golden["cases"][1]["response"].clone();
    let first_candidate_id = duplicate_candidate_ids["candidates"][0]["candidateId"].clone();
    duplicate_candidate_ids["candidates"][1]["candidateId"] = first_candidate_id.clone();
    duplicate_candidate_ids["candidates"][1]["acquisition"]["candidateId"] = first_candidate_id;
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(duplicate_candidate_ids)
            .is_err()
    );

    let mut unknown_response_field = golden["cases"][0]["response"].clone();
    unknown_response_field
        .as_object_mut()
        .unwrap()
        .insert("credential".to_string(), serde_json::json!("secret"));
    assert!(
        serde_json::from_value::<SkillsResolveInstallationSourceResponse>(unknown_response_field)
            .is_err()
    );
    for (field, value) in [
        ("schemaVersion", serde_json::json!(1)),
        ("provider", serde_json::json!("gitlab")),
        ("outcome", serde_json::json!("installed")),
    ] {
        let mut response = golden["cases"][0]["response"].clone();
        response[field] = value;
        assert!(
            serde_json::from_value::<SkillsResolveInstallationSourceResponse>(response).is_err()
        );
    }

    let mut unknown_error_code = golden["errors"][0].clone();
    unknown_error_code["code"] = serde_json::json!("repositoryMoved");
    assert!(serde_json::from_value::<SkillSourceResolutionErrorData>(unknown_error_code).is_err());
    let mut unknown_error_field = golden["errors"][0].clone();
    unknown_error_field
        .as_object_mut()
        .unwrap()
        .insert("internalUrl".to_string(), serde_json::json!("secret"));
    assert!(serde_json::from_value::<SkillSourceResolutionErrorData>(unknown_error_field).is_err());
    for (field, value) in [
        ("phase", "install"),
        ("recovery", "retrySamePreparation"),
        ("provider", "gitlab"),
    ] {
        let mut error = golden["errors"][1].clone();
        error[field] = serde_json::json!(value);
        assert!(serde_json::from_value::<SkillSourceResolutionErrorData>(error).is_err());
    }
}

#[test]
fn skill_installation_workflow_requests_are_strict_discriminated_unions() {
    assert_eq!(
        SKILLS_INSPECT_INSTALLATION_METHOD,
        "skills.inspectInstallation"
    );
    assert_eq!(
        SKILLS_COMMIT_INSTALLATION_METHOD,
        "skills.commitInstallation"
    );
    assert_eq!(SKILLS_CANCEL_PREPARATION_METHOD, "skills.cancelPreparation");
    assert_eq!(SKILLS_LIST_MANAGEMENT_METHOD, "skills.listManagement");
    assert_eq!(SKILLS_SET_ENABLED_METHOD, "skills.setEnabled");
    assert_eq!(SKILLS_CHANGED_NOTIFICATION_METHOD, "skills.changed");

    let no_workspace = serde_json::from_value::<SkillsListRequest>(serde_json::json!({}))
        .expect("projectId is optional");
    assert_eq!(no_workspace.project_id, None);
    let workspace = serde_json::from_value::<SkillsListRequest>(serde_json::json!({
        "projectId": "project-one"
    }))
    .unwrap();
    assert_eq!(workspace.project_id.as_deref(), Some("project-one"));

    assert!(
        serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
            "preparationId": "11111111-1111-4111-8111-111111111111",
            "intent": { "operation": "install", "skillId": "not-allowed" },
            "source": { "kind": "installedSource" }
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
            "preparationId": "11111111-1111-4111-8111-111111111111",
            "intent": { "operation": "install" },
            "source": {
                "kind": "localDirectory",
                "directory": "/tmp/skill",
                "credential": "must-not-cross-the-boundary"
            }
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillsInspectInstallationRequest>(serde_json::json!({
            "preparationId": "11111111-1111-4111-8111-111111111111",
            "intent": { "operation": "install" },
            "source": {
                "kind": "githubRepository",
                "owner": "example",
                "repository": "skills",
                "reference": { "kind": "commit", "sha": "abc", "ref": "main" }
            }
        }))
        .is_err()
    );
    for invalid_source in [
        serde_json::json!({ "kind": "localDirectory", "directory": "  " }),
        serde_json::json!({
            "kind": "githubRepository",
            "owner": "",
            "repository": "skills"
        }),
        serde_json::json!({
            "kind": "githubRepository",
            "owner": "example",
            "repository": "skills",
            "reference": { "kind": "named", "value": " " }
        }),
        serde_json::json!({
            "kind": "githubRepository",
            "owner": "example",
            "repository": "skills",
            "subdirectory": " "
        }),
    ] {
        assert!(serde_json::from_value::<SkillAcquisitionSourceDto>(invalid_source).is_err());
    }
    assert!(
        serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
            "kind": "githubRepository",
            "owner": "example",
            "repository": "skills",
            "reference": { "kind": "named", "value": "main" },
            "subdirectory": "skills/auditor"
        }))
        .is_ok()
    );
    assert!(
        serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
            "kind": "githubRepository",
            "owner": "example",
            "repository": "skills",
            "reference": { "kind": "named", "value": "main" },
            "resolvedCommit": "0123456789abcdef0123456789abcdef01234567"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillAcquisitionSourceDto>(serde_json::json!({
            "kind": "resolvedCandidate",
            "resolutionId": "00000000-0000-0000-0000-000000000000",
            "candidateId": "candidate"
        }))
        .is_err()
    );

    assert!(
        serde_json::from_value::<SkillManagementErrorData>(serde_json::json!({
            "type": "skillManagement",
            "operation": "setEnabled",
            "code": "stateConflict",
            "recovery": "refreshManagement",
            "message": "Refresh the management inventory.",
            "currentStateRevision": "must-not-be-invented"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<SkillManagementErrorData>(serde_json::json!({
            "type": "skillManagement",
            "operation": "setEnabled",
            "code": "stale",
            "recovery": "refreshManagement",
            "message": "Refresh the management inventory."
        }))
        .is_err()
    );
}

#[test]
fn office_status_contract_is_strict_and_uses_the_stable_method() {
    assert_eq!(OFFICE_GET_STATUS_METHOD, "office.getStatus");

    let status = serde_json::from_value::<OfficeEngineStatusDto>(serde_json::json!({
        "schemaVersion": OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
        "providerId": "officecli",
        "availability": "available",
        "source": "packagedComponent",
        "version": "OfficeCLI 1.2.3",
        "engineRevision": "office-engine-sha256-v1:abc",
        "capabilities": {
            "providerId": "officecli",
            "documentKinds": ["document", "spreadsheet", "presentation"],
            "operations": ["help", "create", "validate"],
            "supportsRendering": true,
            "supportsValidation": true,
            "supportsStructuredOutput": true
        }
    }))
    .expect("valid Office status must deserialize");
    assert_eq!(status.availability, OfficeEngineAvailabilityDto::Available);
    assert_eq!(
        status.source,
        Some(OfficeEngineSourceDto::PackagedComponent)
    );

    let mut unknown_field = serde_json::to_value(&status).unwrap();
    unknown_field.as_object_mut().unwrap().insert(
        "executablePath".to_string(),
        serde_json::json!("/secret/path"),
    );
    assert!(serde_json::from_value::<OfficeEngineStatusDto>(unknown_field).is_err());

    let mut unknown_enum = serde_json::to_value(&status).unwrap();
    unknown_enum["availability"] = serde_json::json!("degraded");
    assert!(serde_json::from_value::<OfficeEngineStatusDto>(unknown_enum).is_err());
}

#[test]
fn disabled_skill_activation_has_a_stable_reject_selection_contract() {
    let data = SkillActivationErrorData {
        error_type: "skillActivation",
        code: SkillActivationErrorCodeDto::Disabled,
        recovery: SkillActivationRecoveryDto::RejectSelection,
        message: "The selected Skill is disabled.".to_string(),
        skill_id: Some("installed:user:22222222-2222-4222-8222-222222222222".to_string()),
        expected_revision: None,
        actual_revision: None,
    };
    let value = serde_json::to_value(data).unwrap();

    assert_eq!(value["code"], "disabled");
    assert_eq!(value["recovery"], "rejectSelection");
}

fn assert_wire_round_trip<T>(value: &Value)
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
}

#[test]
fn skill_installation_error_data_is_stable_and_never_contains_a_directory() {
    let data = SkillInstallationErrorData {
        error_type: SkillInstallationErrorTypeDto::SkillInstallation,
        operation: SkillInstallationOperationDto::Update,
        code: SkillInstallationErrorCodeDto::CommitIndeterminate,
        recovery: SkillInstallationRecoveryDto::RetrySameRequest,
        message: "The update may already be visible.".to_string(),
        commit_may_have_succeeded: true,
        installation_id: Some("018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string()),
        skill_id: Some("installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d".to_string()),
        diagnostic_code: None,
        intended_revision: Some("skill-package-sha256-v1:new".to_string()),
        expected_revision: Some("skill-package-sha256-v1:old".to_string()),
        actual_revision: None,
        capacity: None,
        limit: None,
    };

    let value = serde_json::to_value(data).unwrap();

    assert_eq!(SKILL_INSTALLATION_ERROR_CODE, -32010);
    assert_eq!(value["type"], "skillInstallation");
    assert_eq!(value["operation"], "update");
    assert_eq!(value["code"], "commitIndeterminate");
    assert_eq!(value["recovery"], "retrySameRequest");
    assert_eq!(value["commitMayHaveSucceeded"], true);
    assert!(value.get("directory").is_none());
    assert!(value.get("actualRevision").is_none());
}

#[test]
fn action_decision_requests_require_run_scoped_identity() {
    let action: AgentActionIdRequest = serde_json::from_value(serde_json::json!({
        "runId": "run-1",
        "actionId": "call-1"
    }))
    .unwrap();
    assert_eq!(action.run_id, "run-1");
    assert_eq!(action.action_id, "call-1");
    assert!(
        serde_json::from_value::<AgentActionIdRequest>(serde_json::json!({
            "actionId": "call-1"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<AgentRejectActionRequest>(serde_json::json!({
            "actionId": "call-1",
            "message": "no"
        }))
        .is_err()
    );
}
