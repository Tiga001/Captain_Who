use super::*;
use crate::protocol::{
    AgentCommandPermission, AgentPermissions, AgentReadPermission, AgentRunContext,
    AgentWorkspaceContext,
};
use crate::tools::ToolRegistry;
use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn wire(request: Value) -> Value {
    json!({ "request": request })
}

#[test]
fn unified_schema_is_portable_strict_and_has_no_raw_patch_or_revision() {
    let definition = ApplyPatchTool.definition();
    let schema = definition.input_schema;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["request"]));
    assert!(schema.get("oneOf").is_none());
    assert!(schema.get("anyOf").is_none());
    assert!(schema.get("allOf").is_none());
    let request = &schema["properties"]["request"];
    assert_eq!(request["oneOf"].as_array().unwrap().len(), 11);
    assert!(!schema.to_string().contains("\"patch\""));
    assert!(!schema.to_string().contains("expectedRevision"));
    let update_edits = &request["oneOf"][2];
    assert_eq!(
        update_edits["properties"]["edits"]["items"]["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn guidance_preserves_observation_lifecycle_and_staged_settlement() {
    let description = ApplyPatchTool.definition().description;
    for invariant in [
        "create (apply or begin) omits observationId and needs no prior read",
        "atomic no-clobber",
        "success returns its first fileChangeTarget",
        "read the target first if no reusable observation is available",
        "renews the same observationId to the verified post-write state",
        "reuse it only after receiving the successful Tool Result",
        "never for multiple writes in the same Provider Tool Call batch",
        "observationRefreshRequired=true",
        "follow continueWith and read_file again",
        "Failure, rejection, cancellation, conflict and outcome_unknown do not renew",
        "never invent them or replay persisted chunks",
        "follow allowedNextActions",
        "append/edit changes only the draft",
        "only a successful commit issues or renews fileChangeTarget",
        "commit or abort before user-visible narration",
        "Never bypass file-change approval",
    ] {
        assert!(
            description.contains(invariant),
            "missing guidance: {invariant}"
        );
    }

    // The example is executable documentation, not a source of fabricated observation IDs.
    let example = description
        .split_once("Direct create: ")
        .unwrap()
        .1
        .split_once(". For larger content")
        .unwrap()
        .0;
    let args: Value = serde_json::from_str(example).unwrap();
    validate_wire_shape(&args).unwrap();
    assert_eq!(args["request"]["operation"], "create");
    assert!(args["request"].get("observationId").is_none());
}

#[test]
fn create_path_guidance_does_not_require_an_existing_observation() {
    let schema = ApplyPatchTool.definition().input_schema;
    let branches = schema["properties"]["request"]["oneOf"].as_array().unwrap();
    for branch in branches {
        let properties = &branch["properties"];
        let Some(path) = properties.get("filePath") else {
            continue;
        };
        let description = path["description"].as_str().unwrap();
        if properties["operation"]["enum"][0] == "create" {
            assert!(description.contains("New target path"));
            assert!(description.contains("No prior read required"));
            assert!(properties.get("observationId").is_none());
        } else {
            assert!(description.contains("exact fileChangeTarget.filePath"));
            assert!(description.contains("read_file or the latest successful apply/commit"));
            assert!(properties.get("observationId").is_some());
        }
    }
}

#[test]
fn staged_wire_enforces_every_exact_action_matrix() {
    let valid = [
        wire(json!({"action":"begin","operation":"create","filePath":"a.txt"})),
        wire(
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"modify"}),
        ),
        wire(
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
        ),
        wire(
            json!({"action":"append","transactionId":"file-change-staged-v1:x","index":0,"expectedDraftRevision":0,"content":"x"}),
        ),
        wire(
            json!({"action":"edit","transactionId":"file-change-staged-v1:x","index":1,"expectedDraftRevision":1,"edits":[{"kind":"append","text":"y"}]}),
        ),
        wire(
            json!({"action":"commit","transactionId":"file-change-staged-v1:x","expectedDraftRevision":2,"summary":"done"}),
        ),
        wire(json!({"action":"status","transactionId":"file-change-staged-v1:x"})),
        wire(json!({"action":"abort","transactionId":"file-change-staged-v1:x"})),
    ];
    for value in valid {
        validate_wire_shape(&value).unwrap_or_else(|error| panic!("rejected {value}: {error}"));
    }

    let invalid = [
        wire(
            json!({"action":"begin","operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
        ),
        wire(
            json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x","strategy":"rewrite"}),
        ),
        wire(
            json!({"action":"begin","operation":"create","filePath":"a.txt","observationId":"fobs_x"}),
        ),
        wire(
            json!({"action":"begin","operation":"update","filePath":"a.txt","observationId":"fobs_x"}),
        ),
        wire(
            json!({"action":"append","transactionId":"x","index":0,"expectedDraftRevision":null,"content":"x"}),
        ),
        wire(
            json!({"action":"edit","transactionId":"x","index":0,"expectedDraftRevision":0,"edits":[],"content":"x"}),
        ),
        wire(json!({"action":"commit","transactionId":"x","expected_draft_revision":0})),
        wire(json!({"action":"status","transactionId":"x","summary":"extra"})),
        wire(json!({"action":"abort","transactionId":"x","extra":true})),
    ];
    for value in invalid {
        assert!(validate_wire_shape(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn strict_wire_rejects_missing_null_unknown_snake_case_raw_patch_and_bad_combinations() {
    let valid = wire(json!({
        "action": "apply", "operation": "delete", "filePath": "a.txt",
        "observationId": "fobs_example"
    }));
    let invalid = [
        json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example"}),
        json!({"operation":"delete","filePath":"a.txt","observationId":"fobs_example"}),
        wire(
            json!({"action":"apply","operation":"create","filePath":"a.txt","observationId":"fobs_example","content":"x"}),
        ),
        wire(
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":null}),
        ),
        wire(
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","extra":true}),
        ),
        wire(
            json!({"action":"apply","operation":"delete","file_path":"a.txt","observationId":"fobs_example"}),
        ),
        wire(
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","patch":"@@"}),
        ),
        wire(
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","content":"x","edits":[]}),
        ),
        wire(
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"replace","oldText":"a","newText":"b","replaceAll":null}]}),
        ),
        wire(
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"append","text":"x","extra":true}]}),
        ),
        wire(
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_example","edits":[{"kind":"insert_before","text":"x"}]}),
        ),
        wire(
            json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_example","content":"x"}),
        ),
        wire(json!({
            "action":"apply",
            "operation":"delete",
            "filePath":"a.txt",
            "observationId":"fobs_example",
            "summary":"x".repeat(MAX_SUMMARY_CHARS + 1)
        })),
    ];
    assert!(validate_wire_shape(&valid).is_ok());
    for value in invalid {
        assert!(validate_wire_shape(&value).is_err(), "accepted {value}");
    }
}

#[test]
fn registry_entry_returns_stable_typed_errors_for_invalid_direct_json() {
    let workspace = TestWorkspace::new();
    let context = workspace.context();
    let registry = ToolRegistry::defaults_with_search(None);
    let cases = [
        (
            wire(
                json!({"action":1,"operation":"delete","filePath":"a.txt","observationId":"fobs_x"}),
            ),
            "agent.apply_patch.invalid_arguments",
        ),
        (
            wire(
                json!({"action":"apply","operation":"delete","filePath":"","observationId":"fobs_x"}),
            ),
            "agent.apply_patch.invalid_arguments",
        ),
        (
            wire(
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":""}),
            ),
            "agent.apply_patch.invalid_arguments",
        ),
        (
            wire(
                json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"fobs_x","patch":"@@"}),
            ),
            "agent.apply_patch.unknown_field",
        ),
        (
            wire(
                json!({"action":"apply","operation":"delete","filePath":"a.txt","observationId":"fobs_x","summary":null}),
            ),
            "agent.apply_patch.invalid_arguments",
        ),
    ];

    for (args, expected_code) in cases {
        let call = AgentToolCall {
            id: format!("invalid-{}", Uuid::new_v4()),
            tool: "apply_patch".to_string(),
            args,
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = registry.proposed_action(&context, &call).unwrap_err();
        assert_eq!(error.code(), Some(expected_code));
        assert!(!error.to_string().contains("serde"));
        assert!(!error.to_string().contains("structured_edit_error"));
    }
}

#[test]
fn wire_errors_return_safe_exact_and_parseable_corrections() {
    let workspace = TestWorkspace::new();
    let context = workspace.context();
    let registry = ToolRegistry::defaults_with_search(None);

    let missing_observation = wire(json!({
        "action":"apply",
        "operation":"update",
        "filePath":"missing.txt",
        "content":"hello\n"
    }));
    let error = file_change_wire_error(
        validate_wire_shape(&missing_observation).unwrap_err(),
        &missing_observation,
    );
    let continuation = &error.details().unwrap()["continueWith"];
    assert_eq!(continuation["tool"], "read_file");
    assert_eq!(continuation["args"], json!({"path":"missing.txt"}));
    let read = registry.execute(
        &context,
        &AgentToolCall {
            id: "correct-read".to_string(),
            tool: "read_file".to_string(),
            args: continuation["args"].clone(),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
    );
    assert!(read.ok, "{}", read.error.unwrap_or_default());

    let bad_cursor = wire(json!({
        "action":"append",
        "transactionId":"file-change-staged-v1:current",
        "content":"chunk"
    }));
    let error = file_change_wire_error(validate_wire_shape(&bad_cursor).unwrap_err(), &bad_cursor);
    let continuation = &error.details().unwrap()["continueWith"];
    assert_eq!(continuation["tool"], "apply_patch");
    assert_eq!(continuation["args"]["request"]["action"], "status");
    validate_wire_shape(&continuation["args"]).unwrap();
    parse_args(continuation["args"].clone()).unwrap();

    let oversized = wire(json!({
        "action":"apply",
        "operation":"create",
        "filePath":"large.txt",
        "content":"DIRECT_PRIVATE_CANARY".repeat(MAX_INLINE_CONTENT_BYTES)
    }));
    let error = file_change_wire_error(validate_wire_shape(&oversized).unwrap_err(), &oversized);
    let details = error.details().unwrap();
    let continuation = &details["continueWith"];
    assert_eq!(continuation["tool"], "apply_patch");
    assert_eq!(continuation["args"]["request"]["action"], "begin");
    assert_eq!(continuation["args"]["request"]["operation"], "create");
    validate_wire_shape(&continuation["args"]).unwrap();
    parse_args(continuation["args"].clone()).unwrap();
    assert!(!details.to_string().contains("DIRECT_PRIVATE_CANARY"));
    assert!(!details.to_string().contains("<exact filePath>"));
}

#[test]
fn wire_errors_describe_the_exact_branch_without_echoing_body_fields() {
    let invalid_begin = wire(json!({
        "action":"begin",
        "operation":"create",
        "filePath":"new.txt",
        "strategy":"rewrite"
    }));
    let invalid = [
        invalid_begin.clone(),
        wire(json!({
            "action":"edit",
            "transactionId":"file-change-staged-v1:current",
            "index":0,
            "expectedDraftRevision":0,
            "edits":[{"kind":"append","text":"PRIVATE_EDIT_CANARY"}],
            "content":"PRIVATE_EXTRA_CANARY"
        })),
    ];
    for value in invalid {
        let error = file_change_wire_error(validate_wire_shape(&value).unwrap_err(), &value);
        let shape = &error.details().unwrap()["expectedShape"]["request"];
        assert!(shape["allowedFields"].as_array().is_some());
        assert!(!shape.to_string().contains("PRIVATE_EDIT_CANARY"));
        assert!(!shape.to_string().contains("PRIVATE_EXTRA_CANARY"));
    }
    let begin_shape = wire_expected_shape(&invalid_begin);
    assert!(!begin_shape["request"]["allowedFields"]
        .as_array()
        .unwrap()
        .contains(&json!("strategy")));
}

#[test]
fn black_box_failure_shapes_correct_missing_discriminators_and_mixed_edit_fields() {
    // Regression for the manual A07 failure: the model supplied an otherwise exact update
    // edit but omitted `action`. The correction must name the apply/update branch instead of
    // returning only a low-information list of every action.
    let missing_action = wire(json!({
        "operation":"update",
        "filePath":"crlf.txt",
        "observationId":"fobs_example",
        "edits":[{"kind":"replace","oldText":"CRLF_TARGET","newText":"CRLF_REPLACED"}]
    }));
    let shape = wire_expected_shape(&missing_action);
    assert_eq!(shape["request"]["action"], "apply");
    assert_eq!(shape["request"]["operation"], "update");
    assert_eq!(
        shape["request"]["requiredFields"],
        json!(["action", "operation", "filePath", "observationId", "edits"])
    );

    // Regressions for A04/A11: every edit kind advertises its own exact keys, while
    // begin/create explicitly excludes the update-only strategy field.
    let mixed_edit = wire(json!({
        "action":"apply",
        "operation":"update",
        "filePath":"existing.txt",
        "observationId":"fobs_example",
        "edits":[{"kind":"insert_before","anchor":"TARGET_ONCE","newText":"PRIVATE"}]
    }));
    let details =
        file_change_wire_error(validate_wire_shape(&mixed_edit).unwrap_err(), &mixed_edit);
    let expected = &details.details().unwrap()["expectedShape"]["request"];
    assert_eq!(
        expected["editShapes"]["insert_before"]["requiredFields"],
        json!(["kind", "anchor", "text"])
    );
    assert!(!expected.to_string().contains("PRIVATE"));

    let create_with_strategy = wire(json!({
        "action":"begin",
        "operation":"create",
        "filePath":"long-staged.md",
        "strategy":"rewrite"
    }));
    let expected = wire_expected_shape(&create_with_strategy);
    assert_eq!(expected["request"]["action"], "begin");
    assert_eq!(expected["request"]["operation"], "create");
    assert!(!expected["request"]["allowedFields"]
        .as_array()
        .unwrap()
        .contains(&json!("strategy")));
}

#[test]
fn renderer_event_projection_excludes_content_edits_and_observation_authority() {
    let call = AgentToolCall {
        id: "apply-1".to_string(),
        tool: "apply_patch".to_string(),
        args: wire(json!({
            "action":"apply",
            "operation":"update",
            "filePath":"a.txt",
            "observationId":"fobs_secret",
            "edits":[{"kind":"append","text":"secret content"}]
        })),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let projected = ApplyPatchTool.event_call_projection(&call);
    assert!(projected.args["request"].get("observationId").is_none());
    assert_eq!(projected.args["request"]["editCount"], 1);
    assert!(!projected.args.to_string().contains("secret content"));

    let staged = AgentToolCall {
        id: "append-1".to_string(),
        tool: "apply_patch".to_string(),
        args: wire(json!({
            "action":"append",
            "transactionId":"transaction-1",
            "index":0,
            "expectedDraftRevision":0,
            "content":"PRIVATE_STAGED_BODY_CANARY"
        })),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let staged_projection = ApplyPatchTool.event_call_projection(&staged);
    assert_eq!(staged_projection.args["request"]["contentBytes"], 26);
    assert!(staged_projection.args["request"]
        .get("contentDigest")
        .is_some());
    assert!(!staged_projection
        .args
        .to_string()
        .contains("PRIVATE_STAGED_BODY_CANARY"));

    let flat_malformed = AgentToolCall {
        id: "flat-malformed".to_string(),
        tool: "apply_patch".to_string(),
        args: json!({"action":"apply","content":"PRIVATE_FLAT_CANARY"}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let flat_projection = ApplyPatchTool.event_call_projection(&flat_malformed);
    assert_eq!(flat_projection.args, json!({"invalidRequest":true}));
    assert!(!flat_projection
        .args
        .to_string()
        .contains("PRIVATE_FLAT_CANARY"));

    let nested_malformed = AgentToolCall {
        id: "nested-malformed".to_string(),
        tool: "apply_patch".to_string(),
        args: wire(json!({
            "action":"apply",
            "operation":"delete",
            "filePath":"PRIVATE_PATH_CANARY",
            "body":"PRIVATE_NESTED_CANARY"
        })),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let nested_projection = ApplyPatchTool.event_call_projection(&nested_malformed);
    assert_eq!(nested_projection.args, json!({"invalidRequest":true}));
    assert!(!nested_projection
        .args
        .to_string()
        .contains("PRIVATE_NESTED_CANARY"));
    assert!(!nested_projection
        .args
        .to_string()
        .contains("PRIVATE_PATH_CANARY"));
}

#[test]
fn durable_trace_projection_uses_the_authority_digest_shape_without_private_text() {
    let call = AgentToolCall {
        id: "trace-1".to_string(),
        tool: "apply_patch".to_string(),
        args: wire(json!({
            "action":"apply",
            "operation":"create",
            "filePath":"trace.txt",
            "content":"PRIVATE_DURABLE_TRACE_CANARY\n"
        })),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };

    let projected = ApplyPatchTool.trace_call_projection(&call);
    assert_eq!(
        crate::file_change::proposal_digest(&projected.args).unwrap(),
        crate::file_change_support::apply_patch_trace_args_digest(&call.args).unwrap()
    );
    assert_eq!(projected.args["request"]["action"], "apply");
    assert_eq!(projected.args["request"]["changeRepresentation"], "content");
    assert!(projected.args["request"].get("contentDigest").is_some());
    assert!(projected.args["request"].get("observationId").is_none());
    assert!(!projected
        .args
        .to_string()
        .contains("PRIVATE_DURABLE_TRACE_CANARY"));

    let binary_call = AgentToolCall {
        args: wire(json!({
            "action":"apply",
            "operation":"create",
            "filePath":"binary.txt",
            "content":"data:text/plain;base64,UFJJVkFURV9CSU5BUllfQ0FOQVJZ"
        })),
        ..call
    };
    let binary_projection = ApplyPatchTool.trace_call_projection(&binary_call);
    assert_eq!(
        crate::file_change::proposal_digest(&binary_projection.args).unwrap(),
        crate::file_change_support::apply_patch_trace_args_digest(&binary_call.args).unwrap()
    );
    assert!(!binary_projection
        .args
        .to_string()
        .contains("UFJJVkFURV9CSU5BUllfQ0FOQVJZ"));
}

#[test]
fn camel_case_create_update_delete_form_frozen_transactions() {
    let workspace = TestWorkspace::new();
    workspace.write("existing.txt", "alpha\nbeta\n");
    let context = workspace.context();

    let create = proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":"created.txt","content":"created\n"}),
        )
        .unwrap();
    assert_eq!(create.operation, AgentFileChangeOperation::Create);
    assert_eq!(
        create.execution.target_content.as_deref(),
        Some("created\n")
    );
    create.execution.validate().unwrap();
    create.validate().unwrap();
    let mut illegal_approval = create.clone();
    illegal_approval.approval_status = AgentApprovalStatus::NotRequired;
    assert!(illegal_approval.validate().is_err());
    illegal_approval.approval_status = AgentApprovalStatus::Rejected;
    assert!(illegal_approval.validate().is_err());

    let existing = observe(&context, "existing.txt");
    let update = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"existing.txt","observationId":existing,"edits":[{"kind":"replace","oldText":"beta","newText":"gamma"}]}),
        )
        .unwrap();
    assert_eq!(update.operation, AgentFileChangeOperation::Update);
    assert_eq!(
        update.execution.target_content.as_deref(),
        Some("alpha\ngamma\n")
    );
    update.execution.validate().unwrap();
    update.validate().unwrap();

    let existing = observe(&context, "existing.txt");
    let delete = proposal(
            &context,
            json!({"action":"apply","operation":"delete","filePath":"existing.txt","observationId":existing}),
        )
        .unwrap();
    assert_eq!(delete.operation, AgentFileChangeOperation::Delete);
    assert!(delete.execution.target_content.is_none());
    delete.execution.validate().unwrap();
    delete.validate().unwrap();
}

#[test]
fn observation_must_be_exact_and_current() {
    let workspace = TestWorkspace::new();
    workspace.write("a.txt", "before\n");
    workspace.write("b.txt", "other\n");
    let context = workspace.context();
    let observation = observe(&context, "a.txt");
    let missing = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":"missing","content":"after\n"}),
        )
        .unwrap_err();
    assert_eq!(
        missing.code(),
        Some("agent.apply_patch.observation_required")
    );
    let wrong_path = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"b.txt","observationId":observation,"content":"after\n"}),
        )
        .unwrap_err();
    assert_eq!(
        wrong_path.code(),
        Some("agent.apply_patch.observation_path_mismatch")
    );
    workspace.write("a.txt", "changed concurrently\n");
    let stale = proposal(
            &context,
            json!({"action":"apply","operation":"update","filePath":"a.txt","observationId":observation,"content":"after\n"}),
        )
        .unwrap_err();
    assert_eq!(stale.code(), Some("agent.apply_patch.observation_stale"));
}

#[test]
fn sibling_direct_creates_freeze_independent_missing_preconditions() {
    let workspace = TestWorkspace::new();
    let context = workspace.context();
    let first = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":"first.txt",
            "content":"first\n"
        }),
    )
    .unwrap();
    // Freeze the second proposal before the first create is published. Its execution-time
    // missing-target check models a proposal waiting for approval while an authorized sibling
    // FileChange settles in the same Run.
    let second = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":"second.txt",
            "content":"second\n"
        }),
    )
    .unwrap();

    let commit = |proposal: &AgentFileChangeProposal, committed_at| {
        let target = FileChangePathPolicy::new(Some(&workspace.root), false)
            .resolve(&proposal.execution.canonical_target)
            .unwrap();
        proposal
            .execution
            .observation
            .revalidate_current_identity(target.absolute_path())
            .unwrap();
        let plan = crate::file_change::FileChangePlan::from_binding(&proposal.execution)
            .expect("rebuild the frozen Direct plan");
        FileChangeCommitter
            .commit_fresh(
                &proposal.execution.transaction.id,
                &target,
                &plan,
                committed_at,
                None,
            )
            .expect("commit the authorized sibling create");
    };

    commit(&first, 1);
    commit(&second, 2);

    // This proposal is intentionally built only after two sibling entries changed the parent
    // directory. The exact target is still absent, so a fresh proposal remains valid.
    let third = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":"third.txt",
            "content":"third\n"
        }),
    )
    .unwrap();
    commit(&third, 3);

    assert_eq!(
        fs::read_to_string(workspace.root.join("first.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.root.join("second.txt")).unwrap(),
        "second\n"
    );
    assert_eq!(
        fs::read_to_string(workspace.root.join("third.txt")).unwrap(),
        "third\n"
    );
}

#[test]
fn malformed_proposal_does_not_burn_observation_but_valid_proposal_claims_it_once() {
    let workspace = TestWorkspace::new();
    workspace.write("once.txt", "before\n");
    let context = workspace.context();
    let observation = observe(&context, "once.txt");
    let malformed = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"once.txt",
            "observationId":observation,
            "edits":[{"kind":"replace","oldText":"missing","newText":"after"}]
        }),
    )
    .unwrap_err();
    assert_eq!(malformed.code(), Some("agent.apply_patch.match_not_found"));

    proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"once.txt",
            "observationId":observation,
            "content":"after\n"
        }),
    )
    .unwrap();
    let replay = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"once.txt",
            "observationId":observation,
            "content":"another\n"
        }),
    )
    .unwrap_err();
    assert_eq!(
        replay.code(),
        Some("agent.apply_patch.observation_required")
    );
}

#[test]
fn create_existing_returns_safe_file_exists() {
    let workspace = TestWorkspace::new();
    workspace.write("exists.txt", "keep\n");
    let context = workspace.context();
    let error = proposal(
            &context,
            json!({"action":"apply","operation":"create","filePath":"exists.txt","content":"replace\n"}),
        )
        .unwrap_err();
    assert_eq!(error.code(), Some("agent.apply_patch.file_exists"));
    assert_eq!(error.to_string(), "文件已存在。");
}

#[test]
fn no_workspace_absolute_succeeds_and_relative_fails_before_effects() {
    let workspace = TestWorkspace::new();
    let context = workspace.context_without_workspace();
    let absolute = workspace.root.canonicalize().unwrap().join("absolute.txt");
    assert!(proposal(
        &context,
        json!({"action":"apply","operation":"create","filePath":absolute,"content":"ok\n"})
    )
    .is_ok());

    let alias = format!(
        "@home/.mycopilot-file-observation-test-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let alias_direct = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":alias,
            "content":"not published by proposal construction\n"
        }),
    )
    .unwrap();
    assert_eq!(alias_direct.operation, AgentFileChangeOperation::Create);
    assert!(!std::path::Path::new(&alias_direct.execution.canonical_target).exists());

    let relative = proposal(
        &context,
        json!({"action":"apply","operation":"create","filePath":"relative.txt","content":"no\n"}),
    )
    .unwrap_err();
    assert_eq!(
        relative.code(),
        Some("agent.apply_patch.workspace_required")
    );
}

#[cfg(unix)]
#[test]
fn proposal_freeze_cannot_follow_a_swapped_ancestor_or_leaf() {
    let workspace = TestWorkspace::new();
    workspace.write("parent/target.txt", "authorized base\n");
    workspace.write("outside/target.txt", "outside secret\n");
    let context = workspace.context();
    let target = FileChangePathPolicy::new(Some(&workspace.root), true)
        .resolve("parent/target.txt")
        .unwrap();
    let observation_id = observe(&context, "parent/target.txt");
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id().unwrap(),
            context.run_id().unwrap(),
            target.absolute_path(),
        )
        .unwrap();
    let original_parent = workspace.root.join("parent");
    let displaced_parent = workspace.root.join("displaced");
    let outside_parent = workspace.root.join("outside");
    let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
        fs::rename(&original_parent, &displaced_parent).unwrap();
        symlink(&outside_parent, &original_parent).unwrap();
    })
    .unwrap_err();
    assert!(matches!(
        error.code(),
        FileChangeErrorCode::SymlinkForbidden | FileChangeErrorCode::Conflict
    ));
    assert_eq!(
        fs::read_to_string(outside_parent.join("target.txt")).unwrap(),
        "outside secret\n"
    );

    let workspace = TestWorkspace::new();
    workspace.write("target.txt", "authorized base\n");
    workspace.write("outside.txt", "outside secret\n");
    let context = workspace.context();
    let target = FileChangePathPolicy::new(Some(&workspace.root), true)
        .resolve("target.txt")
        .unwrap();
    let observation_id = observe(&context, "target.txt");
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id().unwrap(),
            context.run_id().unwrap(),
            target.absolute_path(),
        )
        .unwrap();
    let leaf = workspace.root.join("target.txt");
    let outside = workspace.root.join("outside.txt");
    let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
        fs::remove_file(&leaf).unwrap();
        symlink(&outside, &leaf).unwrap();
    })
    .unwrap_err();
    assert_eq!(error.code(), FileChangeErrorCode::SymlinkForbidden);
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside secret\n");
}

#[cfg(unix)]
#[test]
fn proposal_freeze_rejects_a_raced_hard_link_before_building_a_diff() {
    let workspace = TestWorkspace::new();
    workspace.write("target.txt", "authorized base\n");
    workspace.write("outside.txt", "outside secret\n");
    let context = workspace.context();
    let target = FileChangePathPolicy::new(Some(&workspace.root), true)
        .resolve("target.txt")
        .unwrap();
    let observation_id = observe(&context, "target.txt");
    let observation = context
        .file_observations()
        .validate(
            &observation_id,
            context.conversation_id().unwrap(),
            context.run_id().unwrap(),
            target.absolute_path(),
        )
        .unwrap();
    let leaf = workspace.root.join("target.txt");
    let outside = workspace.root.join("outside.txt");
    let error = freeze_observed_base_with_hook(&context, &target, &observation, || {
        fs::remove_file(&leaf).unwrap();
        fs::hard_link(&outside, &leaf).unwrap();
    })
    .unwrap_err();
    assert_eq!(error.code(), FileChangeErrorCode::HardLinkForbidden);
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside secret\n");
}

#[test]
fn successful_apply_renews_the_read_observation_for_direct_follow_up_without_reading() {
    let workspace = TestWorkspace::new();
    workspace.write("target.txt", "first version\n");
    workspace.write("sibling.txt", "before\n");
    let context = workspace.context();
    let predecessor = observe(&context, "target.txt");
    let call = apply_call(
        "apply-successor-update",
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"target.txt",
            "observationId":predecessor,
            "content":"second version\n"
        }),
    );
    let frozen_proposal =
        direct_proposal_from_call(&context.clone().with_tool_call_id(call.id.clone()), &call)
            .unwrap();
    workspace.write("target.txt", "second version\n");
    let mut authoritative = successful_result(
        &call,
        AgentFileChangeOperation::Update,
        "target.txt",
        Some(content_revision(b"second version\n")),
    );
    authoritative.result.as_mut().unwrap()["transactionId"] =
        json!(frozen_proposal.transaction_id.clone());
    let mut model = authoritative.clone();

    assert!(attach_successor_observation_to_model_result_with_proposal(
        &context,
        &call,
        &authoritative,
        &mut model,
        &frozen_proposal,
    ));
    let successor = model.result.as_ref().unwrap()["observationId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(successor, predecessor);
    assert_eq!(
        model.result.as_ref().unwrap()["fileChangeTarget"],
        json!({
            "filePath":"target.txt",
            "observationId":successor,
            "state":"existing"
        })
    );

    let mut replayed_model = authoritative.clone();
    assert!(attach_successor_observation_to_model_result_with_proposal(
        &context,
        &call,
        &authoritative,
        &mut replayed_model,
        &frozen_proposal,
    ));
    assert_eq!(
        replayed_model.result.as_ref().unwrap()["observationId"],
        successor,
        "replaying the same settled Tool Call must return the same successor authority"
    );

    let mut reconciled = authoritative.clone();
    reconciled.result.as_mut().unwrap()["status"] = json!("already_applied");
    let mut reconciled_model = reconciled.clone();
    assert!(attach_successor_observation_to_model_result_with_proposal(
        &context,
        &call,
        &reconciled,
        &mut reconciled_model,
        &frozen_proposal,
    ));
    assert_eq!(
        reconciled_model.result.as_ref().unwrap()["observationId"],
        successor,
        "commit-unknown reconciliation must retain the same verified successor"
    );

    // A sibling change is unrelated to the successor's exact target binding.
    workspace.write("sibling.txt", "after\n");
    let follow_up_call = apply_call(
        "apply-successor-follow-up",
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"target.txt",
            "observationId":successor,
            "edits":[{"kind":"append","text":"third line\n"}]
        }),
    );
    let follow_up = direct_proposal_from_call(
        &context.clone().with_tool_call_id(follow_up_call.id.clone()),
        &follow_up_call,
    )
    .unwrap();
    assert_eq!(
        follow_up.execution.transaction.base.revision(),
        Some(content_revision(b"second version\n").as_str())
    );

    workspace.write("target.txt", "second version\nthird line\n");
    let follow_up_result = successful_result(
        &follow_up_call,
        AgentFileChangeOperation::Update,
        "target.txt",
        Some(content_revision(b"second version\nthird line\n")),
    );
    let mut follow_up_model = follow_up_result.clone();
    assert!(attach_successor_observation_to_model_result(
        &context,
        &follow_up_call,
        &follow_up_result,
        &mut follow_up_model,
    ));
    let second_successor = follow_up_model.result.as_ref().unwrap()["observationId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(second_successor, successor);

    let third = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"target.txt",
            "observationId":second_successor,
            "edits":[{"kind":"append","text":"fourth line\n"}]
        }),
    )
    .unwrap();
    assert_eq!(
        third.execution.transaction.base.revision(),
        Some(content_revision(b"second version\nthird line\n").as_str())
    );
}

#[test]
fn successful_delete_renews_the_same_observation_but_create_needs_no_id() {
    let workspace = TestWorkspace::new();
    workspace.write("deleted.txt", "delete me\n");
    let context = workspace.context();
    let predecessor = observe(&context, "deleted.txt");
    let call = apply_call(
        "apply-successor-delete",
        json!({
            "action":"apply",
            "operation":"delete",
            "filePath":"deleted.txt",
            "observationId":predecessor
        }),
    );
    let frozen_proposal =
        direct_proposal_from_call(&context.clone().with_tool_call_id(call.id.clone()), &call)
            .unwrap();
    fs::remove_file(workspace.root.join("deleted.txt")).unwrap();
    let mut authoritative =
        successful_result(&call, AgentFileChangeOperation::Delete, "deleted.txt", None);
    authoritative.result.as_mut().unwrap()["transactionId"] =
        json!(frozen_proposal.transaction_id.clone());
    let mut model = authoritative.clone();

    assert!(attach_successor_observation_to_model_result_with_proposal(
        &context,
        &call,
        &authoritative,
        &mut model,
        &frozen_proposal,
    ));
    let successor = model.result.as_ref().unwrap()["observationId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(successor, predecessor);
    assert_eq!(
        model.result.as_ref().unwrap()["fileChangeTarget"]["state"],
        "missing"
    );
    let follow_up = proposal(
        &context,
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":"deleted.txt",
            "content":"recreated\n"
        }),
    )
    .unwrap();
    assert_eq!(follow_up.operation, AgentFileChangeOperation::Create);
}

#[test]
fn successor_requires_the_exact_call_and_frozen_file_change_binding() {
    let workspace = TestWorkspace::new();
    workspace.write("target.txt", "before\n");
    workspace.write("other.txt", "other\n");
    let context = workspace.context();
    let observation_id = observe(&context, "target.txt");
    let call = apply_call(
        "apply-successor-bound",
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"target.txt",
            "observationId":observation_id,
            "content":"after\n"
        }),
    );
    let proposal =
        direct_proposal_from_call(&context.clone().with_tool_call_id(call.id.clone()), &call)
            .unwrap();
    workspace.write("target.txt", "after\n");
    let mut authoritative = successful_result(
        &call,
        AgentFileChangeOperation::Update,
        "target.txt",
        Some(content_revision(b"after\n")),
    );
    authoritative.result.as_mut().unwrap()["transactionId"] =
        json!(proposal.transaction_id.clone());
    let mut model = authoritative.clone();
    assert!(attach_successor_observation_to_model_result_with_proposal(
        &context,
        &call,
        &authoritative,
        &mut model,
        &proposal,
    ));

    for mutate in [
        |value: &mut Value| value["filePath"] = json!("other.txt"),
        |value: &mut Value| value["transactionId"] = json!("file-change-direct-v1:tampered"),
    ] {
        let mut mismatched = authoritative.clone();
        mutate(mismatched.result.as_mut().unwrap());
        let mut mismatched_model = mismatched.clone();
        assert!(!attach_successor_observation_to_model_result_with_proposal(
            &context,
            &call,
            &mismatched,
            &mut mismatched_model,
            &proposal,
        ));
        assert!(mismatched_model
            .result
            .as_ref()
            .unwrap()
            .get("observationId")
            .is_none());
    }
}

#[test]
fn staged_commit_successor_is_reusable_but_a_post_commit_race_requires_reading() {
    let workspace = TestWorkspace::new();
    workspace.write("staged.txt", "committed draft\n");
    let context = workspace.context();
    let call = apply_call(
        "apply-successor-commit",
        json!({
            "action":"commit",
            "transactionId":"file-change-staged-v1:successor",
            "expectedDraftRevision":2
        }),
    );
    let authoritative = successful_result(
        &call,
        AgentFileChangeOperation::Create,
        "staged.txt",
        Some(content_revision(b"committed draft\n")),
    );
    let mut model = authoritative.clone();
    assert!(attach_successor_observation_to_model_result(
        &context,
        &call,
        &authoritative,
        &mut model,
    ));

    let raced_call = apply_call(
        "apply-successor-raced",
        json!({
            "action":"apply",
            "operation":"update",
            "filePath":"staged.txt",
            "observationId":"fobs_consumed_input",
            "content":"expected target\n"
        }),
    );
    let raced_result = successful_result(
        &raced_call,
        AgentFileChangeOperation::Update,
        "staged.txt",
        Some(content_revision(b"expected target\n")),
    );
    let mut raced_model = raced_result.clone();
    assert!(!attach_successor_observation_to_model_result(
        &context,
        &raced_call,
        &raced_result,
        &mut raced_model,
    ));
    let output = raced_model.result.as_ref().unwrap();
    assert!(output.get("observationId").is_none());
    assert_eq!(output["observationRefreshRequired"], true);
    assert_eq!(output["continueWith"]["tool"], "read_file");
    assert_eq!(output["continueWith"]["args"]["path"], "staged.txt");
}

#[test]
fn successor_observation_is_model_and_private_checkpoint_only() {
    let workspace = TestWorkspace::new();
    workspace.write("private.txt", "current\n");
    let context = workspace.context();
    let registry = ToolRegistry::defaults_with_search(None);
    let call = apply_call(
        "apply-successor-private",
        json!({
            "action":"apply",
            "operation":"create",
            "filePath":"private.txt",
            "content":"current\n"
        }),
    );
    let authoritative = successful_result(
        &call,
        AgentFileChangeOperation::Create,
        "private.txt",
        Some(content_revision(b"current\n")),
    );
    let mut model = registry.model_projection(&authoritative);
    assert!(attach_successor_observation_to_model_result(
        &context,
        &call,
        &authoritative,
        &mut model,
    ));
    let mut checkpoint = registry.checkpoint_projection(&authoritative);
    copy_successor_observation_projection(&model, &mut checkpoint);
    assert!(model
        .result
        .as_ref()
        .unwrap()
        .get("observationId")
        .is_some());
    assert!(checkpoint
        .result
        .as_ref()
        .unwrap()
        .get("observationId")
        .is_some());
    for projected in [
        registry.event_projection(&model),
        registry.trace_projection(&model),
        registry.archive_projection(&model),
    ] {
        let output = projected.result.as_ref().unwrap();
        assert!(output.get("observationId").is_none());
        assert!(output.get("fileChangeTarget").is_none());
    }
}

fn apply_call(id: &str, request: Value) -> AgentToolCall {
    AgentToolCall {
        id: id.to_string(),
        tool: "apply_patch".to_string(),
        args: wire(request),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    }
}

fn successful_result(
    call: &AgentToolCall,
    operation: AgentFileChangeOperation,
    file_path: &str,
    revision: Option<String>,
) -> AgentToolResult {
    let result = AgentFileChangeResult {
        schema_version: AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: AgentFileChangeResultStatus::Applied,
        outcome: crate::protocol::AgentFileChangeOutcome::Applied,
        transaction_id: if apply_patch_action(&call.args) == Some("commit") {
            call.args["request"]["transactionId"]
                .as_str()
                .unwrap()
                .to_string()
        } else {
            format!("file-change-direct-v1:{}", call.id)
        },
        operation,
        update_strategy: None,
        file_path: file_path.to_string(),
        additions: 1,
        deletions: u64::from(operation == AgentFileChangeOperation::Update),
        line_count: if operation == AgentFileChangeOperation::Delete {
            0
        } else {
            1
        },
        byte_count: if operation == AgentFileChangeOperation::Delete {
            0
        } else {
            1
        },
        revision,
        error_code: None,
        error: None,
        message: Some("文件变更已应用。".to_string()),
    };
    result.validate().unwrap();
    AgentToolResult {
        exact_archive_file: None,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: true,
        result: Some(serde_json::to_value(result).unwrap()),
        error: None,
    }
}

fn observe(context: &ToolExecutionContext, path: &str) -> String {
    let result = ToolRegistry::defaults_with_search(None).execute(
        context,
        &AgentToolCall {
            id: format!("read-{}", Uuid::new_v4()),
            tool: "read_file".to_string(),
            args: json!({"path":path}),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
    );
    assert!(result.ok, "{}", result.error.unwrap_or_default());
    result.result.unwrap()["observationId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn proposal(context: &ToolExecutionContext, args: Value) -> AgentResult<AgentFileChangeProposal> {
    let call_id = format!("apply-{}", Uuid::new_v4());
    let bound_context = context.clone().with_tool_call_id(call_id.clone());
    direct_proposal_from_call(
        &bound_context,
        &AgentToolCall {
            id: call_id,
            tool: "apply_patch".to_string(),
            args: wire(args),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    )
}

struct TestWorkspace {
    root: std::path::PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "mycopilot-direct-file-change-{}",
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn context(&self) -> ToolExecutionContext {
        self.context_with_workspace(Some(self.root.clone()))
    }

    fn context_without_workspace(&self) -> ToolExecutionContext {
        self.context_with_workspace(None)
    }

    fn context_with_workspace(&self, root: Option<std::path::PathBuf>) -> ToolExecutionContext {
        ToolExecutionContext::from_run_context(Some(&AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("direct-test-conversation".to_string()),
            project_id: None,
            workspace: root.map(|root| AgentWorkspaceContext {
                folders: Vec::new(),
                project_id: None,
                display_name: Some("test".to_string()),
                root_path: Some(root.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: Default::default(),
                builtin_execution: Default::default(),
            },
        }))
        .with_runtime_services("direct-test-run".to_string(), None)
        .with_file_change_tool_set_revision("tool-set-test-v1".to_string())
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
