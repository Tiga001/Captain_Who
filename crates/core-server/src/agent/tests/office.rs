use super::*;
use mycopilot_core::office::{
    OfficeDocumentKind, OfficeEngine, OfficeEngineAvailability, OfficeEngineCapabilities,
    OfficeEngineErrorCode, OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus,
    OfficeExecutionContext, OfficeExecutionRequest, OfficeFileState, OfficeFrozenPath,
    OfficeOperation, OfficeOperationAccess, OfficeOperationParameters, OfficePathIdentity,
    OfficePathPurpose, OfficePathScope, OfficePathSlot, OfficePreparedExecution,
    OfficeRequestParameters, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Barrier;

#[derive(Clone)]
struct FailedOfficeEngine;

impl OfficeEngine for FailedOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        OfficeEngineCapabilities {
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            document_kinds: vec![
                OfficeDocumentKind::Document,
                OfficeDocumentKind::Spreadsheet,
                OfficeDocumentKind::Presentation,
            ],
            operations: vec![OfficeOperation::Set],
            supports_rendering: true,
            supports_validation: true,
            supports_structured_output: true,
        }
    }

    fn status(&self, _cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: OfficeEngineAvailability::Available,
            source: Some(OfficeEngineSource::Configured),
            version: Some("test".to_string()),
            engine_revision: Some("office-engine-test".to_string()),
            capabilities: self.capabilities(),
            error_code: None,
            message: None,
        }
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        unreachable!("the Host executes only the approval-frozen plan")
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        Ok(OfficeExecutionResult {
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            engine_revision: prepared.engine_revision.clone(),
            document_kind: prepared.request.document_kind,
            operation: prepared.request.operation,
            argv: prepared.argv.clone(),
            cwd: ".".to_string(),
            exit_code: Some(7),
            stdout: "partial-output\n".to_string(),
            stderr: "provider-error\n".to_string(),
            timed_out: false,
            cancelled: false,
            duration_ms: 12,
            stdout_truncated: false,
            stderr_truncated: false,
            error_code: None,
            error: None,
        })
    }
}

#[derive(Clone)]
struct SuccessfulTrackingOfficeEngine {
    executions: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct RevalidationFailureOfficeEngine;

impl OfficeEngine for RevalidationFailureOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        FailedOfficeEngine.capabilities()
    }

    fn status(&self, cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        FailedOfficeEngine.status(cancellation)
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        unreachable!("the Host executes only the approval-frozen plan")
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        _prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        Err(OfficeEngineError::new(
            OfficeEngineErrorCode::PreconditionFailed,
            OfficeEngineRecovery::Retry,
            "The workbook changed after approval.",
        ))
    }
}

impl OfficeEngine for SuccessfulTrackingOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        FailedOfficeEngine.capabilities()
    }

    fn status(&self, cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        FailedOfficeEngine.status(cancellation)
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        unreachable!("the Host executes only the approval-frozen plan")
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(OfficeExecutionResult {
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            engine_revision: prepared.engine_revision.clone(),
            document_kind: prepared.request.document_kind,
            operation: prepared.request.operation,
            argv: prepared.argv.clone(),
            cwd: ".".to_string(),
            exit_code: Some(0),
            stdout: "provider-success\n".to_string(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 8,
            stdout_truncated: false,
            stderr_truncated: false,
            error_code: None,
            error: None,
        })
    }
}

fn test_path_identity(revision: &str) -> OfficePathIdentity {
    OfficePathIdentity {
        revision: revision.to_string(),
        device: Some(1),
        inode: Some(2),
    }
}

fn prepared_spreadsheet_operation() -> OfficePreparedExecution {
    OfficePreparedExecution {
        schema_version: OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: "office-engine-test".to_string(),
        workspace_revision: Some("office-workspace-test".to_string()),
        access: OfficeOperationAccess::FileWrite,
        request: OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Spreadsheet,
            operation: OfficeOperation::Set,
            document_path: Some("budget.xlsx".to_string()),
            parameters: OfficeRequestParameters::Typed(OfficeOperationParameters::Set {
                target: "/Sheet1/A1".to_string(),
                properties: BTreeMap::from([("value".to_string(), serde_json::json!(42))]),
                replacement: None,
                force: false,
            }),
            output_path: None,
            destination_path: None,
            timeout_ms: Some(5_000),
        },
        argv: vec![
            "set".to_string(),
            "budget.xlsx".to_string(),
            "/Sheet1/A1".to_string(),
            "--prop".to_string(),
            "value=42".to_string(),
        ],
        paths: vec![OfficeFrozenPath {
            slot: OfficePathSlot::Document,
            logical_path: "budget.xlsx".to_string(),
            purpose: OfficePathPurpose::InPlaceTarget,
            scope: OfficePathScope::Workspace,
            normalized_path: "/workspace/budget.xlsx".to_string(),
            state: OfficeFileState::Present,
            object_identity: Some(test_path_identity("office-file-identity-test")),
            parent_identity: test_path_identity("office-parent-identity-test"),
            content_revision: Some("office-file-test".to_string()),
            size: Some(128),
            write_disposition: Some(OfficeWriteDisposition::ReplaceExisting),
        }],
        document_precondition: None,
        output_precondition: None,
        destination_precondition: None,
        resource_preconditions: Vec::new(),
    }
}

fn automatic_office_input(workspace: &Path) -> AgentChatInput {
    let mut input = command_test_input(workspace);
    input
        .context
        .as_mut()
        .expect("Office test run context")
        .permissions
        .patch = mycopilot_core::AgentPatchPermission::AutoApprove;
    input
}

fn prepared_office_action(id: &str) -> AgentProposedAction {
    AgentProposedAction::OfficeOperation {
        office_operation: Box::new(mycopilot_core::AgentOfficeOperationRequest {
            schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
            id: id.to_string(),
            prepared: prepared_spreadsheet_operation(),
            approval_status: AgentApprovalStatus::Approved,
            reason: "Update the approved workbook cell".to_string(),
        }),
    }
}

#[test]
fn approved_office_failure_preserves_exit_stdout_and_stderr() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage).with_office_engine(Arc::new(FailedOfficeEngine));
    let input = command_test_input(&workspace);
    let operation = mycopilot_core::AgentOfficeOperationRequest {
        schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
        id: "office-call-1".to_string(),
        prepared: prepared_spreadsheet_operation(),
        approval_status: AgentApprovalStatus::Approved,
        reason: "Update the approved workbook cell".to_string(),
    };

    let result =
        service.execute_office_operation(&input, &operation, AgentCancellationToken::new(), None);

    assert!(!result.ok);
    assert_eq!(result.tool, "office_spreadsheet");
    assert_eq!(result.result.as_ref().unwrap()["exitCode"], 7);
    assert_eq!(
        result.result.as_ref().unwrap()["stdout"],
        "partial-output\n"
    );
    assert_eq!(
        result.result.as_ref().unwrap()["stderr"],
        "provider-error\n"
    );
    assert!(result.error.as_deref().unwrap().contains("code 7"));
}

#[test]
fn office_revalidation_failure_preserves_frozen_execution_context() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service =
        AgentService::new(storage).with_office_engine(Arc::new(RevalidationFailureOfficeEngine));
    let input = command_test_input(fixture.path());
    let operation = match prepared_office_action("office-revalidation-failure") {
        AgentProposedAction::OfficeOperation { office_operation } => office_operation,
        _ => unreachable!(),
    };

    let result =
        service.execute_office_operation(&input, &operation, AgentCancellationToken::new(), None);

    let evidence = result.result.as_ref().unwrap();
    assert!(!result.ok);
    assert_eq!(result.call_id, "office-revalidation-failure");
    assert_eq!(evidence["code"], "office.precondition_failed");
    assert_eq!(evidence["recovery"], "retry");
    assert_eq!(evidence["providerId"], OFFICECLI_PROVIDER_ID);
    assert_eq!(evidence["engineRevision"], "office-engine-test");
    assert_eq!(evidence["documentKind"], "spreadsheet");
    assert_eq!(evidence["operation"], "set");
    assert_eq!(evidence["argv"][0], "set");
    assert_eq!(evidence["cwd"], ".");
    assert!(evidence["exitCode"].is_null());
    assert_eq!(evidence["stdout"], "");
    assert_eq!(evidence["stderr"], "");
    assert_eq!(evidence["timedOut"], false);
    assert_eq!(evidence["cancelled"], false);
    assert_eq!(evidence["errorCode"], "office.precondition_failed");
    assert_eq!(evidence["error"], "The workbook changed after approval.");
}

#[test]
fn legacy_office_action_envelope_is_rejected_before_provider_execution() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service =
        AgentService::new(storage).with_office_engine(Arc::new(SuccessfulTrackingOfficeEngine {
            executions: Arc::clone(&executions),
        }));
    let input = command_test_input(fixture.path());
    let mut operation = match prepared_office_action("office-legacy-envelope") {
        AgentProposedAction::OfficeOperation { office_operation } => office_operation,
        _ => unreachable!(),
    };
    operation.schema_version = 1;

    let result =
        service.execute_office_operation(&input, &operation, AgentCancellationToken::new(), None);

    assert!(!result.ok);
    assert_eq!(result.call_id, "office-legacy-envelope");
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "invalidApprovedSnapshot"
    );
    assert_eq!(result.result.as_ref().unwrap()["recovery"], "retry");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn host_rejects_noncanonical_or_overlong_frozen_office_reasons_before_execution() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service =
        AgentService::new(storage).with_office_engine(Arc::new(SuccessfulTrackingOfficeEngine {
            executions: Arc::clone(&executions),
        }));
    let input = command_test_input(fixture.path());

    for (index, reason) in [
        String::new(),
        " reason with surrounding whitespace ".to_string(),
        "界".repeat(mycopilot_core::AGENT_OFFICE_REASON_MAX_CHARS + 1),
        "Inspect\nthe workbook".to_string(),
        "Inspect\u{0007}the workbook".to_string(),
        "Inspect\u{202e}the workbook".to_string(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut operation = match prepared_office_action(&format!("office-invalid-reason-{index}"))
        {
            AgentProposedAction::OfficeOperation { office_operation } => office_operation,
            _ => unreachable!(),
        };
        operation.reason = reason;

        let result = service.execute_office_operation(
            &input,
            &operation,
            AgentCancellationToken::new(),
            None,
        );

        assert!(!result.ok);
        assert_eq!(
            result.result.as_ref().unwrap()["code"],
            "invalidApprovedSnapshot"
        );
    }
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn office_action_helpers_expose_stable_identity_without_executable_path() {
    let request = mycopilot_core::AgentOfficeOperationRequest {
        schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
        id: "office-call-2".to_string(),
        prepared: prepared_spreadsheet_operation(),
        approval_status: AgentApprovalStatus::Required,
        reason: "Update budget".to_string(),
    };
    let action = AgentProposedAction::OfficeOperation {
        office_operation: Box::new(request),
    };

    assert_eq!(action_id_for_action(&action), "office-call-2");
    assert_eq!(action_type_for_action(&action), "office_operation");
    assert_eq!(tool_name_for_action(&action), "office_spreadsheet");
    let call = tool_call_for_action(&action);
    assert_eq!(call.tool, "office_spreadsheet");
    assert_eq!(call.args["request"]["operation"], "set");
    assert_eq!(call.args["request"]["filePath"], "budget.xlsx");
    assert_eq!(call.args["request"]["target"], "/Sheet1/A1");
    assert_eq!(call.args["request"]["properties"]["value"], 42);
    assert_eq!(call.args["reason"], "Update budget");
    assert!(call.args.get("operation").is_none());
    assert!(call.args.get("parameters").is_none());
    assert!(call.args.get("arguments").is_none());
    assert!(call.args["request"].get("parameters").is_none());
    assert!(call.args["request"].get("arguments").is_none());
    assert!(serde_json::to_string(&call)
        .unwrap()
        .contains("office_spreadsheet"));
    assert!(!serde_json::to_string(&call).unwrap().contains("officecli"));
}

#[test]
fn automatic_office_authorization_failure_returns_paired_structured_tool_result() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new(storage);
    let input = command_test_input(fixture.path());

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input,
                "run-office-authorization-denied".to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action("office-authorization-denied"),
            AgentCancellationToken::new(),
        )
        .expect("a Host policy rejection must remain a paired tool result");

    assert!(!result.ok);
    assert_eq!(result.call_id, "office-authorization-denied");
    assert_eq!(result.tool, "office_spreadsheet");
    assert_eq!(result.result.as_ref().unwrap()["type"], "file_write_policy");
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "explicitApprovalRequired"
    );
}

#[test]
fn automatic_office_execution_stops_before_side_effect_when_executing_audit_fails() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service =
        AgentService::new(storage).with_office_engine(Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }));
    let run_id = "run-office-executing-audit-failure";
    let call_id = "office-executing-audit-failure";
    inject_auto_action_audit_failure(run_id, call_id, "executing");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(!result.ok);
    assert_eq!(result.call_id, call_id);
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "auditPersistenceFailed"
    );
    assert_eq!(result.result.as_ref().unwrap()["phase"], "beforeExecution");
    assert_eq!(result.result.as_ref().unwrap()["executionAttempted"], false);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn automatic_office_final_audit_failure_never_reports_success_and_preserves_execution() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service =
        AgentService::new(storage).with_office_engine(Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }));
    let run_id = "run-office-final-audit-failure";
    let call_id = "office-final-audit-failure";
    inject_auto_action_audit_failure(run_id, call_id, "completed");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();

    let evidence = result.result.as_ref().unwrap();
    assert!(!result.ok);
    assert_eq!(result.call_id, call_id);
    assert_eq!(evidence["code"], "auditPersistenceFailed");
    assert_eq!(evidence["phase"], "afterExecution");
    assert_eq!(evidence["recovery"], "inspectState");
    assert_eq!(evidence["executionAttempted"], true);
    assert_eq!(evidence["commitMayHaveSucceeded"], true);
    assert_eq!(evidence["execution"]["ok"], true);
    assert_eq!(evidence["execution"]["result"]["exitCode"], 0);
    assert_eq!(
        evidence["execution"]["result"]["stdout"],
        "provider-success\n"
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[test]
fn automatic_office_reconciles_a_terminal_receipt_after_post_commit_error() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new(Arc::clone(&storage)).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
    let run_id = "run-office-post-commit-reconciliation";
    let call_id = "office-post-commit-reconciliation";
    inject_auto_action_audit_post_commit_failure(run_id, call_id, "completed");

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let audited = storage
        .list_agent_tool_results_for_run(run_id, "office_spreadsheet")
        .unwrap();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].call_id, result.call_id);
    assert_eq!(audited[0].tool, result.tool);
    assert_eq!(audited[0].ok, result.ok);
    assert_eq!(audited[0].result, result.result);
    assert_eq!(audited[0].error, result.error);
}

#[test]
fn automatic_office_success_persists_one_final_tool_result_audit() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new(Arc::clone(&storage)).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
    let run_id = "run-office-success-audit";
    let call_id = "office-success-audit";

    let result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();

    assert!(result.ok);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let audited = storage
        .list_agent_tool_results_for_run(run_id, "office_spreadsheet")
        .unwrap();
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].call_id, call_id);
    assert_eq!(audited[0].tool, "office_spreadsheet");
    assert!(audited[0].ok);
    assert_eq!(audited[0].result.as_ref().unwrap()["exitCode"], 0);

    let replay = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&replay).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_automatic_office_replay_is_at_most_once_and_never_conflicts() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new(Arc::clone(&storage)).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
    let run_id = "run-office-concurrent-claim";
    let call_id = "office-concurrent-claim";
    let barrier = Arc::new(Barrier::new(3));
    let workspace = fixture.path().to_path_buf();

    let handles = (0..2)
        .map(|_| {
            let service = service.clone();
            let barrier = Arc::clone(&barrier);
            let workspace = workspace.clone();
            std::thread::spawn(move || {
                barrier.wait();
                service.execute_auto_approved_action(
                    AutoApprovedActionContext::new(
                        automatic_office_input(&workspace),
                        run_id.to_string(),
                        None,
                        None,
                        None,
                    ),
                    prepared_office_action(call_id),
                    AgentCancellationToken::new(),
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let returned_results = results
        .iter()
        .filter_map(|outcome| outcome.as_ref().ok())
        .collect::<Vec<_>>();
    assert!(!returned_results.is_empty());
    for result in &returned_results {
        assert!(result.ok);
        assert_eq!(result.call_id, call_id);
        assert_eq!(result.tool, "office_spreadsheet");
    }
    if returned_results.len() == 2 {
        assert_eq!(
            serde_json::to_value(returned_results[0]).unwrap(),
            serde_json::to_value(returned_results[1]).unwrap()
        );
    }
    for error in results.iter().filter_map(|outcome| outcome.as_ref().err()) {
        assert_eq!(error.code(), Some("agent.office_execution_claim_rejected"));
        assert_eq!(error.details().unwrap()["code"], "executionAlreadyClaimed");
        assert_eq!(error.details().unwrap()["recovery"], "inspectState");
    }
    assert_eq!(
        storage
            .list_agent_tool_results_for_run(run_id, "office_spreadsheet")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn later_authority_change_cannot_overwrite_or_contradict_completed_office_receipt() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new(Arc::clone(&storage)).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
    let run_id = "run-office-rejection-after-completion";
    let call_id = "office-rejection-after-completion";

    let completed = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                automatic_office_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap();
    assert!(completed.ok);

    let conflict = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                command_test_input(fixture.path()),
                run_id.to_string(),
                None,
                None,
                None,
            ),
            prepared_office_action(call_id),
            AgentCancellationToken::new(),
        )
        .unwrap_err();
    assert_eq!(
        conflict.code(),
        Some("agent.office_execution_claim_rejected")
    );
    assert_eq!(
        conflict.details().unwrap()["code"],
        "actionIdentityConflict"
    );

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let audited = storage
        .list_agent_tool_results_for_run(run_id, "office_spreadsheet")
        .unwrap();
    assert_eq!(audited.len(), 1);
    assert!(audited[0].ok);
    assert_eq!(audited[0].result.as_ref().unwrap()["exitCode"], 0);
}

#[test]
fn office_audit_path_scope_summarizes_every_frozen_path_purpose_and_scope() {
    let input = command_test_input(Path::new("/workspace"));
    let mut prepared = prepared_spreadsheet_operation();
    prepared.paths.push(OfficeFrozenPath {
        slot: OfficePathSlot::Destination,
        logical_path: "@documents/report.xlsx".to_string(),
        purpose: OfficePathPurpose::WriteTarget,
        scope: OfficePathScope::External,
        normalized_path: "/home/test/Documents/report.xlsx".to_string(),
        state: OfficeFileState::Missing,
        object_identity: None,
        parent_identity: test_path_identity("external-parent"),
        content_revision: None,
        size: None,
        write_disposition: Some(OfficeWriteDisposition::CreateNew),
    });
    prepared.paths.push(OfficeFrozenPath {
        slot: OfficePathSlot::Resource { index: 0 },
        logical_path: "@attachments/logo.png".to_string(),
        purpose: OfficePathPurpose::ReadSource,
        scope: OfficePathScope::Attachment,
        normalized_path: "/attachments/logo.png".to_string(),
        state: OfficeFileState::Present,
        object_identity: Some(test_path_identity("attachment-object")),
        parent_identity: test_path_identity("attachment-parent"),
        content_revision: Some("attachment-content".to_string()),
        size: Some(42),
        write_disposition: None,
    });
    let action = AgentProposedAction::OfficeOperation {
        office_operation: Box::new(mycopilot_core::AgentOfficeOperationRequest {
            schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
            id: "office-path-audit".to_string(),
            prepared,
            approval_status: AgentApprovalStatus::Approved,
            reason: "Audit Office path scopes".to_string(),
        }),
    };

    let summary = path_scope_for_action(&input, &action).expect("Office path audit summary");
    let summary: Value = serde_json::from_str(&summary).unwrap();
    assert_eq!(summary["totalPathCount"], 3);
    assert_eq!(summary["truncated"], false);
    assert_eq!(summary["paths"][0]["purpose"], "inPlaceTarget");
    assert_eq!(summary["paths"][0]["scope"], "workspace");
    assert_eq!(summary["paths"][1]["purpose"], "writeTarget");
    assert_eq!(summary["paths"][1]["scope"], "external");
    assert_eq!(summary["paths"][2]["purpose"], "readSource");
    assert_eq!(summary["paths"][2]["scope"], "attachment");
}

#[test]
fn rejected_office_action_publishes_exactly_one_paired_tool_result_event() {
    let action = prepared_office_action("office-rejected-event");
    let tool_result = AgentToolResult {
        call_id: "office-rejected-event".to_string(),
        tool: "office_spreadsheet".to_string(),
        ok: true,
        result: Some(json!({ "status": "rejected" })),
        error: None,
    };
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    assert!(
        super::super::approval::publish_inline_file_write_tool_result(
            &notifications,
            "run-office-rejected-event",
            &action,
            AgentApprovalDecisionStatus::Rejected,
            &tool_result,
        )
    );
    // An approved Office action is published by its asynchronous executor. The inline path must
    // refuse it even if future control-flow changes accidentally reach this helper.
    assert!(
        !super::super::approval::publish_inline_file_write_tool_result(
            &notifications,
            "run-office-rejected-event",
            &action,
            AgentApprovalDecisionStatus::Approved,
            &tool_result,
        )
    );

    let event = receiver.try_recv().expect("one rejection ToolResult event");
    assert_eq!(event["params"]["type"], "tool_result");
    assert_eq!(event["params"]["runId"], "run-office-rejected-event");
    assert_eq!(event["params"]["result"]["callId"], "office-rejected-event");
    assert_eq!(event["params"]["result"]["tool"], "office_spreadsheet");
    assert!(receiver.try_recv().is_err());
}
