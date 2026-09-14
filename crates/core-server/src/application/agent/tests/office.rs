use super::*;
use mycopilot_core::office::{
    OfficeDocumentKind, OfficeEngine, OfficeEngineAvailability, OfficeEngineCapabilities,
    OfficeEngineErrorCode, OfficeEngineRecovery, OfficeEngineSource, OfficeEngineStatus,
    OfficeExecutionContext, OfficeExecutionRequest, OfficeFileState, OfficeFrozenPath,
    OfficeGridLayout, OfficeManagedScriptBinding, OfficeManagedScriptPurpose, OfficeOperation,
    OfficeOperationAccess, OfficeOperationParameters, OfficePathIdentity, OfficePathPurpose,
    OfficePathScope, OfficePathSlot, OfficePreparedExecution, OfficePublishedOutput,
    OfficePublishedOutputKind, OfficePublishedOutputRole, OfficeRenderGridGeometry,
    OfficeRenderLayoutCoverage, OfficeRenderLayoutEvidence, OfficeRenderPageSelection,
    OfficeViewMode, OfficeViewRenderMode, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION,
    OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
use mycopilot_core::skills::SkillResourceSession;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Barrier;

#[cfg(unix)]
fn write_probe_officecli(path: &Path, version: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(
        path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf '{version}\\n'; exit 0; fi\nexit 0\n"
        ),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[derive(Clone)]
struct LifecycleTestOfficeEngine {
    revision: &'static str,
    invalid_status: bool,
    invalid_prepare: bool,
    invalid_execute: bool,
    invalid_presentation_edit: bool,
    status_calls: Arc<AtomicUsize>,
    prepare_calls: Arc<AtomicUsize>,
    executions: Arc<AtomicUsize>,
    presentation_edits: Arc<AtomicUsize>,
}

impl LifecycleTestOfficeEngine {
    fn valid(revision: &'static str) -> Self {
        Self {
            revision,
            invalid_status: false,
            invalid_prepare: false,
            invalid_execute: false,
            invalid_presentation_edit: false,
            status_calls: Arc::new(AtomicUsize::new(0)),
            prepare_calls: Arc::new(AtomicUsize::new(0)),
            executions: Arc::new(AtomicUsize::new(0)),
            presentation_edits: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn invalid_configuration(message: &'static str) -> OfficeEngineError {
        OfficeEngineError::new(
            OfficeEngineErrorCode::InvalidConfiguration,
            OfficeEngineRecovery::Retry,
            message,
        )
    }
}

impl OfficeEngine for LifecycleTestOfficeEngine {
    fn capabilities(&self) -> OfficeEngineCapabilities {
        FailedOfficeEngine.capabilities()
    }

    fn status(&self, _cancellation: AgentCancellationToken) -> OfficeEngineStatus {
        self.status_calls.fetch_add(1, Ordering::SeqCst);
        OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: if self.invalid_status {
                OfficeEngineAvailability::Unavailable
            } else {
                OfficeEngineAvailability::Available
            },
            source: Some(OfficeEngineSource::Configured),
            version: Some("test".to_string()),
            engine_revision: Some(self.revision.to_string()),
            capabilities: self.capabilities(),
            error_code: self.invalid_status.then(|| {
                OfficeEngineErrorCode::InvalidConfiguration
                    .stable_name()
                    .to_string()
            }),
            message: self
                .invalid_status
                .then(|| "The Office component changed after discovery.".to_string()),
        }
    }

    fn prepare(
        &self,
        _context: &OfficeExecutionContext,
        _request: &OfficeExecutionRequest,
    ) -> Result<OfficePreparedExecution, OfficeEngineError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        if self.invalid_prepare {
            return Err(Self::invalid_configuration(
                "The Office component changed before preparation.",
            ));
        }
        let mut prepared = prepared_spreadsheet_operation();
        prepared.engine_revision = self.revision.to_string();
        Ok(prepared)
    }

    fn execute_prepared(
        &self,
        _context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        if self.invalid_execute {
            return Err(Self::invalid_configuration(
                "The Office component changed before execution.",
            ));
        }
        if prepared.engine_revision != self.revision {
            return Err(OfficeEngineError::new(
                OfficeEngineErrorCode::PreconditionFailed,
                OfficeEngineRecovery::Retry,
                "The prepared Office operation belongs to an older engine revision.",
            ));
        }
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(OfficeExecutionResult {
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            engine_revision: self.revision.to_string(),
            document_kind: prepared.request.document_kind,
            operation: prepared.request.operation,
            argv: prepared.argv.clone(),
            cwd: ".".to_string(),
            exit_code: Some(0),
            stdout: "provider-success\n".to_string(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 1,
            stdout_truncated: false,
            stderr_truncated: false,
            error_code: None,
            error: None,
            outputs: Vec::new(),
        })
    }

    fn execute_presentation_edit(
        &self,
        _context: &OfficeExecutionContext,
        request: &OfficePresentationEditRequest,
        cancellation: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficePresentationEditResult, OfficeEngineError> {
        self.presentation_edits.fetch_add(1, Ordering::SeqCst);
        if self.invalid_presentation_edit {
            return Err(Self::invalid_configuration(
                "The Office component changed before presentation edit settlement.",
            ));
        }
        let cancelled = cancellation.is_cancelled()
            || action_cancel_flag
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst));
        Ok(OfficePresentationEditResult {
            exit_code: (!cancelled).then_some(0),
            stdout: request.destination_path.clone(),
            stderr: String::new(),
            timed_out: false,
            cancelled,
            duration_ms: 1,
            error_code: None,
            error: None,
        })
    }
}

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
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
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
            // A buggy provider must not be able to attach a success artifact to a failed result.
            outputs: vec![test_published_render_output()],
        })
    }
}

#[derive(Clone)]
struct SuccessfulTrackingOfficeEngine {
    executions: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct SkillSessionTrackingOfficeEngine {
    executions_with_runtime_session: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct PublishedRenderOfficeEngine;

#[derive(Clone)]
struct RevalidationFailureOfficeEngine;

fn test_published_render_output() -> OfficePublishedOutput {
    OfficePublishedOutput {
        role: OfficePublishedOutputRole::Render,
        kind: OfficePublishedOutputKind::Image,
        mime_type: "image/png".to_string(),
        source: mycopilot_core::AgentFileInputRef::Workspace {
            path: "preview.png".to_string(),
        },
        read_path: "preview.png".to_string(),
        scope: OfficePathScope::Workspace,
        readable_by_agent: true,
        size_bytes: 128,
        sha256: "ab".repeat(32),
        width: Some(640),
        height: Some(360),
        page_count: None,
        source_sha256: None,
        renderer_revision: None,
        page_selection: OfficeRenderPageSelection::Explicit { pages: vec![1] },
        layout_coverage: Some(OfficeRenderLayoutCoverage {
            requested_pages: vec![1],
            evidence: OfficeRenderLayoutEvidence::TrustedRendererGeometry,
            grid: Some(OfficeRenderGridGeometry {
                columns: 1,
                rows: 1,
                viewport_width: 640,
                viewport_height: 360,
                content_width: 640,
                content_height: 360,
            }),
        }),
    }
}

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
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
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
            outputs: Vec::new(),
        })
    }
}

impl OfficeEngine for SkillSessionTrackingOfficeEngine {
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
        context: &OfficeExecutionContext,
        prepared: &OfficePreparedExecution,
        _cancellation: AgentCancellationToken,
        _action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        if format!("{context:?}").contains("skill_resource_session: Some(0)") {
            self.executions_with_runtime_session
                .fetch_add(1, Ordering::SeqCst);
        }
        Ok(OfficeExecutionResult {
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
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
            duration_ms: 1,
            stdout_truncated: false,
            stderr_truncated: false,
            error_code: None,
            error: None,
            outputs: Vec::new(),
        })
    }
}

impl OfficeEngine for PublishedRenderOfficeEngine {
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
        Ok(OfficeExecutionResult {
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            engine_revision: prepared.engine_revision.clone(),
            document_kind: prepared.request.document_kind,
            operation: prepared.request.operation,
            argv: prepared.argv.clone(),
            cwd: ".".to_string(),
            exit_code: Some(0),
            stdout: "rendered\n".to_string(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 9,
            stdout_truncated: false,
            stderr_truncated: false,
            error_code: None,
            error: None,
            outputs: vec![test_published_render_output()],
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
            parameters: OfficeOperationParameters::Set {
                target: "/Sheet1/A1".to_string(),
                properties: BTreeMap::from([("value".to_string(), serde_json::json!(42))]),
                replacement: None,
                force: false,
            },
            output_path: None,
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: Some(5_000),
        },
        argv: vec![
            "set".to_string(),
            "budget.xlsx".to_string(),
            "/Sheet1/A1".to_string(),
            "--prop".to_string(),
            "value=42".to_string(),
        ],
        resolved_render_plan: None,
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
        input_bindings: Vec::new(),
    }
}

fn prepared_document_render_operation() -> OfficePreparedExecution {
    OfficePreparedExecution {
        schema_version: OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: "office-engine-test".to_string(),
        workspace_revision: Some("office-workspace-test".to_string()),
        access: OfficeOperationAccess::FileWrite,
        request: OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Document,
            operation: OfficeOperation::View,
            document_path: Some("sample.docx".to_string()),
            parameters: OfficeOperationParameters::View {
                mode: OfficeViewMode::Screenshot,
                start: None,
                end: None,
                max_lines: None,
                issue_type: None,
                limit: None,
                columns: Vec::new(),
                pages: Vec::new(),
                range: None,
                viewport: None,
                grid: Some(OfficeGridLayout::Auto),
                render_mode: Some(OfficeViewRenderMode::Auto),
                page_count: false,
            },
            output_path: Some("preview.png".to_string()),
            destination_path: None,
            inputs: Vec::new(),
            timeout_ms: Some(5_000),
        },
        argv: vec![
            "view".to_string(),
            "sample.docx".to_string(),
            "--mode".to_string(),
            "screenshot".to_string(),
            "--grid".to_string(),
            "auto".to_string(),
            "--render-mode".to_string(),
            "auto".to_string(),
            "--json".to_string(),
            "--output".to_string(),
            "preview.png".to_string(),
        ],
        resolved_render_plan: None,
        paths: vec![
            OfficeFrozenPath {
                slot: OfficePathSlot::Document,
                logical_path: "sample.docx".to_string(),
                purpose: OfficePathPurpose::ReadSource,
                scope: OfficePathScope::Workspace,
                normalized_path: "/workspace/sample.docx".to_string(),
                state: OfficeFileState::Present,
                object_identity: Some(test_path_identity("office-render-input-identity")),
                parent_identity: test_path_identity("office-render-input-parent"),
                content_revision: Some("office-render-input-revision".to_string()),
                size: Some(256),
                write_disposition: None,
            },
            OfficeFrozenPath {
                slot: OfficePathSlot::Output,
                logical_path: "preview.png".to_string(),
                purpose: OfficePathPurpose::WriteTarget,
                scope: OfficePathScope::Workspace,
                normalized_path: "/workspace/preview.png".to_string(),
                state: OfficeFileState::Missing,
                object_identity: None,
                parent_identity: test_path_identity("office-render-output-parent"),
                content_revision: None,
                size: None,
                write_disposition: Some(OfficeWriteDisposition::CreateNew),
            },
        ],
        input_bindings: Vec::new(),
    }
}

fn semantic_spreadsheet_operation_args(reason: &str) -> Value {
    serde_json::json!({
        "operation": "writeCell",
        "filePath": "budget.xlsx",
        "sheetName": "Sheet1",
        "cell": "A1",
        "value": 42,
        "timeoutMs": 5_000,
        "reason": reason
    })
}

fn semantic_document_render_args(reason: &str) -> Value {
    serde_json::json!({
        "operation": "render",
        "filePath": "sample.docx",
        "outputPath": "preview.png",
        "timeoutMs": 5_000,
        "reason": reason
    })
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
            semantic_args: semantic_spreadsheet_operation_args("Update the approved workbook cell"),
            prepared: prepared_spreadsheet_operation(),
            approval_status: AgentApprovalStatus::Approved,
            reason: "Update the approved workbook cell".to_string(),
        }),
    }
}

fn prepared_render_action(id: &str) -> AgentProposedAction {
    let reason = "Render the approved document preview";
    AgentProposedAction::OfficeOperation {
        office_operation: Box::new(mycopilot_core::AgentOfficeOperationRequest {
            schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
            id: id.to_string(),
            semantic_args: semantic_document_render_args(reason),
            prepared: prepared_document_render_operation(),
            approval_status: AgentApprovalStatus::Approved,
            reason: reason.to_string(),
        }),
    }
}

fn presentation_edit_request() -> OfficePresentationEditRequest {
    OfficePresentationEditRequest {
        source_path: "source.pptx".to_string(),
        source_binding: mycopilot_core::AgentFileInputBinding {
            schema_version: mycopilot_core::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: "source.pptx".to_string(),
            source: mycopilot_core::AgentFileInputRef::Workspace {
                path: "source.pptx".to_string(),
            },
            size_bytes: 128,
            sha256: "0".repeat(64),
        },
        destination_path: "edited.pptx".to_string(),
        destination_binding: OfficeManagedScriptBinding {
            schema_version: OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION,
            document_kind: OfficeDocumentKind::Presentation,
            purpose: OfficeManagedScriptPurpose::EditPresentationPlan,
            script_mount_path: "__mycopilot/presentation-editor/editor.mjs".to_string(),
            source_mount_path: Some("source.pptx".to_string()),
            destination: OfficeFrozenPath {
                slot: OfficePathSlot::Destination,
                logical_path: "edited.pptx".to_string(),
                purpose: OfficePathPurpose::WriteTarget,
                scope: OfficePathScope::Workspace,
                normalized_path: "/workspace/edited.pptx".to_string(),
                state: OfficeFileState::Missing,
                object_identity: None,
                parent_identity: test_path_identity("office-edit-output-parent"),
                content_revision: None,
                size: None,
                write_disposition: Some(OfficeWriteDisposition::CreateNew),
            },
        },
        inputs: Vec::new(),
        input_bindings: Vec::new(),
        operations: Vec::new(),
        timeout_ms: Some(30_000),
    }
}

#[test]
fn office_status_rediscovery_replaces_a_stale_engine_once() {
    let stale = LifecycleTestOfficeEngine {
        invalid_status: true,
        ..LifecycleTestOfficeEngine::valid("office-engine-v1")
    };
    let replacement = LifecycleTestOfficeEngine::valid("office-engine-v2");
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver: OfficeEngineResolver = {
        let replacement = replacement.clone();
        let resolver_calls = Arc::clone(&resolver_calls);
        Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(replacement.clone())
        })
    };
    let engine = RefreshableOfficeEngine::with_current(Arc::new(stale.clone()), resolver);

    let first = engine.status(AgentCancellationToken::new());
    let second = engine.status(AgentCancellationToken::new());

    assert_eq!(first.availability, OfficeEngineAvailability::Available);
    assert_eq!(first.engine_revision.as_deref(), Some("office-engine-v2"));
    assert_eq!(second.engine_revision.as_deref(), Some("office-engine-v2"));
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stale.status_calls.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.status_calls.load(Ordering::SeqCst), 2);
}

#[cfg(unix)]
#[test]
fn office_status_rediscovery_recovers_after_executable_inode_replacement() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("officecli");
    write_probe_officecli(&executable, "OfficeCLI test-v1");
    let options = OfficeCliDiscoveryOptions::new().with_configured_executable(&executable);
    let stale = resolve_office_engine(&options);
    let before = stale.status(AgentCancellationToken::new());
    assert_eq!(before.availability, OfficeEngineAvailability::Available);
    assert_eq!(before.version.as_deref(), Some("OfficeCLI test-v1"));

    let replacement_path = directory.path().join("officecli.next");
    write_probe_officecli(&replacement_path, "OfficeCLI test-v2");
    fs::rename(&replacement_path, &executable).unwrap();
    let stale_status = stale.status(AgentCancellationToken::new());
    assert_eq!(
        stale_status.availability,
        OfficeEngineAvailability::Unavailable
    );
    assert_eq!(
        stale_status.error_code.as_deref(),
        Some(OfficeEngineErrorCode::InvalidConfiguration.stable_name())
    );

    let resolver: OfficeEngineResolver = Arc::new(move || resolve_office_engine(&options));
    let engine = RefreshableOfficeEngine::with_current(stale, resolver);
    let recovered = engine.status(AgentCancellationToken::new());

    assert_eq!(recovered.availability, OfficeEngineAvailability::Available);
    assert_eq!(recovered.version.as_deref(), Some("OfficeCLI test-v2"));
    assert_ne!(recovered.engine_revision, before.engine_revision);
}

#[test]
fn concurrent_office_preparation_uses_one_rediscovery_for_a_stale_instance() {
    let stale = LifecycleTestOfficeEngine {
        invalid_prepare: true,
        ..LifecycleTestOfficeEngine::valid("office-engine-v1")
    };
    let replacement = LifecycleTestOfficeEngine::valid("office-engine-v2");
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver: OfficeEngineResolver = {
        let replacement = replacement.clone();
        let resolver_calls = Arc::clone(&resolver_calls);
        Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(replacement.clone())
        })
    };
    let engine = Arc::new(RefreshableOfficeEngine::with_current(
        Arc::new(stale),
        resolver,
    ));
    let barrier = Arc::new(Barrier::new(9));
    let context = OfficeExecutionContext::from_run_context(None);
    let request = prepared_spreadsheet_operation().request;
    let mut workers = Vec::new();
    for _ in 0..8 {
        let engine = Arc::clone(&engine);
        let barrier = Arc::clone(&barrier);
        let context = context.clone();
        let request = request.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            engine.prepare(&context, &request).unwrap().engine_revision
        }));
    }
    barrier.wait();

    for worker in workers {
        assert_eq!(worker.join().unwrap(), "office-engine-v2");
    }
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn presentation_edit_is_forwarded_to_the_current_engine_with_host_cancellation() {
    let current = LifecycleTestOfficeEngine::valid("office-engine-v1");
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver: OfficeEngineResolver = {
        let current = current.clone();
        let resolver_calls = Arc::clone(&resolver_calls);
        Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(current.clone())
        })
    };
    let engine = RefreshableOfficeEngine::with_current(Arc::new(current.clone()), resolver);
    let action_cancel_flag = Arc::new(AtomicBool::new(true));

    let result = engine
        .execute_presentation_edit(
            &OfficeExecutionContext::from_run_context(None),
            &presentation_edit_request(),
            AgentCancellationToken::new(),
            Some(action_cancel_flag),
        )
        .expect("the current engine handles the editor transaction");

    assert!(result.cancelled);
    assert_eq!(result.stdout, "edited.pptx");
    assert_eq!(current.presentation_edits.load(Ordering::SeqCst), 1);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn presentation_edit_invalid_configuration_refreshes_without_replaying_the_transaction() {
    let stale = LifecycleTestOfficeEngine {
        invalid_presentation_edit: true,
        ..LifecycleTestOfficeEngine::valid("office-engine-v1")
    };
    let replacement = LifecycleTestOfficeEngine::valid("office-engine-v2");
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver: OfficeEngineResolver = {
        let replacement = replacement.clone();
        let resolver_calls = Arc::clone(&resolver_calls);
        Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(replacement.clone())
        })
    };
    let engine = RefreshableOfficeEngine::with_current(Arc::new(stale.clone()), resolver);

    let error = engine
        .execute_presentation_edit(
            &OfficeExecutionContext::from_run_context(None),
            &presentation_edit_request(),
            AgentCancellationToken::new(),
            None,
        )
        .expect_err("an approved editor transaction must not cross engine revisions");

    assert_eq!(error.code(), OfficeEngineErrorCode::PreconditionFailed);
    assert_eq!(error.recovery(), OfficeEngineRecovery::Retry);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stale.presentation_edits.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.presentation_edits.load(Ordering::SeqCst), 0);

    let retry = engine
        .execute_presentation_edit(
            &OfficeExecutionContext::from_run_context(None),
            &presentation_edit_request(),
            AgentCancellationToken::new(),
            None,
        )
        .expect("a new transaction may use the refreshed engine");
    assert_eq!(retry.exit_code, Some(0));
    assert_eq!(replacement.presentation_edits.load(Ordering::SeqCst), 1);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn approved_office_action_is_not_replayed_across_engine_rediscovery() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let stale = LifecycleTestOfficeEngine {
        invalid_execute: true,
        ..LifecycleTestOfficeEngine::valid("office-engine-test")
    };
    let replacement = LifecycleTestOfficeEngine::valid("office-engine-v2");
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver: OfficeEngineResolver = {
        let replacement = replacement.clone();
        let resolver_calls = Arc::clone(&resolver_calls);
        Arc::new(move || {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            Arc::new(replacement.clone())
        })
    };
    let service = AgentService::new_authorized_for_test(storage).with_office_engine(Arc::new(
        RefreshableOfficeEngine::with_current(Arc::new(stale), resolver),
    ));
    let operation = match prepared_office_action("office-stale-approved-action") {
        AgentProposedAction::OfficeOperation { office_operation } => office_operation,
        _ => unreachable!(),
    };

    let result = service.execute_office_operation(
        &automatic_office_input(fixture.path()),
        &operation,
        None,
        AgentCancellationToken::new(),
        None,
    );

    assert!(!result.ok);
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "office.precondition_failed"
    );
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    assert_eq!(replacement.executions.load(Ordering::SeqCst), 0);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("did not replay the stale prepared or approved operation"));
}

#[test]
fn approved_office_failure_preserves_exit_stdout_and_stderr() {
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage)
        .with_office_engine(Arc::new(FailedOfficeEngine));
    let input = command_test_input(&workspace);
    let operation = mycopilot_core::AgentOfficeOperationRequest {
        schema_version: mycopilot_core::AGENT_OFFICE_OPERATION_SCHEMA_VERSION,
        id: "office-call-1".to_string(),
        semantic_args: semantic_spreadsheet_operation_args("Update the approved workbook cell"),
        prepared: prepared_spreadsheet_operation(),
        approval_status: AgentApprovalStatus::Approved,
        reason: "Update the approved workbook cell".to_string(),
    };

    let result = service.execute_office_operation(
        &input,
        &operation,
        None,
        AgentCancellationToken::new(),
        None,
    );

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
    assert!(result.result.as_ref().unwrap().get("outputs").is_none());
    assert!(result.error.as_deref().unwrap().contains("code 7"));
}

#[test]
fn approved_render_tool_result_preserves_authoritative_published_output() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage)
        .with_office_engine(Arc::new(PublishedRenderOfficeEngine));
    let operation = match prepared_render_action("office-render-output") {
        AgentProposedAction::OfficeOperation { office_operation } => office_operation,
        _ => unreachable!(),
    };

    let result = service.execute_office_operation(
        &automatic_office_input(fixture.path()),
        &operation,
        None,
        AgentCancellationToken::new(),
        None,
    );

    assert!(result.ok, "{:?}", result.error);
    assert_eq!(result.call_id, "office-render-output");
    assert_eq!(result.tool, "office_document");
    let output = &result.result.as_ref().unwrap()["outputs"][0];
    assert_eq!(output["role"], "render");
    assert_eq!(output["kind"], "image");
    assert_eq!(output["mimeType"], "image/png");
    assert_eq!(
        output["source"],
        serde_json::json!({ "type": "workspace", "path": "preview.png" })
    );
    assert_eq!(output["readPath"], "preview.png");
    assert_eq!(output["scope"], "workspace");
    assert_eq!(output["readableByAgent"], true);
    assert_eq!(output["sizeBytes"], 128);
    assert_eq!(output["sha256"], "ab".repeat(32));
    assert_eq!(output["width"], 640);
    assert_eq!(output["height"], 360);
    assert_eq!(
        output["pageSelection"],
        serde_json::json!({ "type": "explicit", "pages": [1] })
    );
    assert_eq!(
        output["layoutCoverage"],
        serde_json::json!({
            "requestedPages": [1],
            "evidence": "trustedRendererGeometry",
            "grid": {
                "columns": 1,
                "rows": 1,
                "viewportWidth": 640,
                "viewportHeight": 360,
                "contentWidth": 640,
                "contentHeight": 360
            }
        })
    );
}

#[test]
fn office_revalidation_failure_preserves_frozen_execution_context() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage)
        .with_office_engine(Arc::new(RevalidationFailureOfficeEngine));
    let input = command_test_input(fixture.path());
    let operation = match prepared_office_action("office-revalidation-failure") {
        AgentProposedAction::OfficeOperation { office_operation } => office_operation,
        _ => unreachable!(),
    };

    let result = service.execute_office_operation(
        &input,
        &operation,
        None,
        AgentCancellationToken::new(),
        None,
    );

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
fn host_rejects_noncanonical_or_overlong_frozen_office_reasons_before_execution() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new_authorized_for_test(storage).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: Arc::clone(&executions),
        },
    ));
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
            None,
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
        semantic_args: semantic_spreadsheet_operation_args("Update budget"),
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
    let AgentProposedAction::OfficeOperation { office_operation } = action else {
        unreachable!()
    };
    assert_eq!(office_operation.semantic_args["operation"], "writeCell");
    assert_eq!(office_operation.semantic_args["filePath"], "budget.xlsx");
    assert_eq!(office_operation.semantic_args["sheetName"], "Sheet1");
    assert_eq!(office_operation.semantic_args["cell"], "A1");
    assert_eq!(office_operation.semantic_args["value"], 42);
    assert_eq!(office_operation.semantic_args["reason"], "Update budget");
    let encoded = serde_json::to_string(&office_operation.semantic_args).unwrap();
    assert!(!encoded.contains("officecli"));
}

#[test]
fn automatic_office_authorization_failure_returns_paired_structured_tool_result() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage);
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
    assert_eq!(
        result.result.as_ref().unwrap()["type"],
        "file_change_policy"
    );
    assert_eq!(
        result.result.as_ref().unwrap()["code"],
        "explicitApprovalRequired"
    );
}

#[test]
fn automatic_dynamic_and_manual_restored_skill_sessions_share_office_execution_context() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let observed = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new_authorized_for_test(storage).with_office_engine(Arc::new(
        SkillSessionTrackingOfficeEngine {
            executions_with_runtime_session: Arc::clone(&observed),
        },
    ));
    let input = automatic_office_input(fixture.path());

    let auto_result = service
        .execute_auto_approved_action(
            AutoApprovedActionContext::new(
                input.clone(),
                "run-office-dynamic-skill-session".to_string(),
                None,
                None,
                Some(Arc::new(SkillResourceSession::empty())),
            ),
            prepared_office_action("office-dynamic-skill-session"),
            AgentCancellationToken::new(),
        )
        .expect("automatic Office execution must preserve the live runtime Skill session");
    assert!(auto_result.ok, "{:?}", auto_result.error);

    let AgentProposedAction::OfficeOperation { office_operation } =
        prepared_office_action("office-manual-restored-skill-session")
    else {
        unreachable!("fixture always creates an Office action")
    };
    let manual_result = service.execute_office_operation(
        &input,
        &office_operation,
        Some(Arc::new(SkillResourceSession::empty())),
        AgentCancellationToken::new(),
        None,
    );
    assert!(manual_result.ok, "{:?}", manual_result.error);
    assert_eq!(
        observed.load(Ordering::SeqCst),
        2,
        "automatic live activation and manual checkpoint restoration must reach the same Host Office context"
    );
}

#[test]
fn automatic_office_execution_stops_before_side_effect_when_executing_audit_fails() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let executions = Arc::new(AtomicUsize::new(0));
    let service = AgentService::new_authorized_for_test(storage).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
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
    let service = AgentService::new_authorized_for_test(storage).with_office_engine(Arc::new(
        SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        },
    ));
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage)).with_office_engine(
        Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }),
    );
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage)).with_office_engine(
        Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }),
    );
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage)).with_office_engine(
        Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }),
    );
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
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage)).with_office_engine(
        Arc::new(SuccessfulTrackingOfficeEngine {
            executions: executions.clone(),
        }),
    );
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
            semantic_args: semantic_spreadsheet_operation_args("Audit Office path scopes"),
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
        exact_archive_file: None,
        call_id: "office-rejected-event".to_string(),
        tool: "office_spreadsheet".to_string(),
        ok: true,
        result: Some(json!({ "status": "rejected" })),
        error: None,
    };
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();

    assert!(
        super::super::approval::publish_inline_file_change_tool_result(
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
        !super::super::approval::publish_inline_file_change_tool_result(
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
