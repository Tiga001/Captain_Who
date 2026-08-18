use super::*;

#[cfg(unix)]
#[test]
fn word_and_excel_python_builders_and_editors_publish_real_wrapper_outputs() {
    let Some(python) = office_editor_test_python() else {
        eprintln!(
            "skipping real Word/Excel Builder/Editor wrapper test: set MYCOPILOT_OFFICE_TEST_PYTHON to Python with python-docx and openpyxl"
        );
        return;
    };

    struct Case {
        profile: AgentCommandRuntimeProfile,
        kind: crate::office::OfficeDocumentKind,
        source: &'static str,
        destination: &'static str,
        template: &'static str,
        builder_destination: &'static str,
        builder_template: &'static str,
        create_source: &'static str,
        verify_source: &'static str,
        verify_output: &'static str,
        verify_builder: &'static str,
    }
    let cases = [
        Case {
            profile: AgentCommandRuntimeProfile::Documents,
            kind: crate::office::OfficeDocumentKind::Document,
            source: "source.docx",
            destination: "edited.docx",
            template: include_str!("../../../skills/bundled/documents/templates/editor.py"),
            builder_destination: "created.docx",
            builder_template: include_str!("../../../skills/bundled/documents/templates/builder.py"),
            create_source: "from docx import Document; import sys; d=Document(); d.add_paragraph('source'); d.save(sys.argv[1])",
            verify_source: "from docx import Document; import sys; assert Document(sys.argv[1]).paragraphs[0].text == 'source'",
            verify_output: "from docx import Document; import sys; assert Document(sys.argv[1]).paragraphs[0].text == 'source-updated'",
            verify_builder: "from docx import Document; import sys; d=Document(sys.argv[1]); assert d.paragraphs[0].text == 'Document title'",
        },
        Case {
            profile: AgentCommandRuntimeProfile::Spreadsheets,
            kind: crate::office::OfficeDocumentKind::Spreadsheet,
            source: "source.xlsx",
            destination: "edited.xlsx",
            template: include_str!("../../../skills/bundled/spreadsheets/templates/editor.py"),
            builder_destination: "created.xlsx",
            builder_template: include_str!("../../../skills/bundled/spreadsheets/templates/builder.py"),
            create_source: "from openpyxl import Workbook; import sys; w=Workbook(); w.active['A1']='source'; w.save(sys.argv[1])",
            verify_source: "from openpyxl import load_workbook; import sys; w=load_workbook(sys.argv[1]); assert w.active['A1'].value == 'source'; assert w.active['A2'].value is None; w.close()",
            verify_output: "from openpyxl import load_workbook; import sys; w=load_workbook(sys.argv[1]); assert w.active['A1'].value == 'source'; assert w.active['A2'].value == 'updated'; w.close()",
            verify_builder: "from openpyxl import load_workbook; import sys; w=load_workbook(sys.argv[1], data_only=False); assert w.sheetnames == ['数据']; assert w['数据']['E2'].value == '=SUM(B2:D2)'; w.close()",
        },
    ];

    for case in cases {
        let workspace = TempDir::new().unwrap();
        fs::create_dir_all(workspace.path().join("scripts")).unwrap();
        let source_path = workspace.path().join(case.source);
        let created = std::process::Command::new(&python)
            .args(["-c", case.create_source])
            .arg(&source_path)
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "source creation failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );

        let editor_source = match case.profile {
            AgentCommandRuntimeProfile::Documents => case.template.replace(
                "raise RuntimeError(\"Replace the EDIT REGION with the requested document edits\")",
                "def transformed(value):\n        return f\"{value}-updated\"\n    for paragraph in document.paragraphs:\n        if paragraph.text == \"source\":\n            for run in paragraph.runs:\n                run.text = transformed(run.text)",
            ),
            AgentCommandRuntimeProfile::Spreadsheets => case.template.replace(
                "raise RuntimeError(\"Replace the EDIT REGION with the requested workbook edits\")",
                "def updates():\n        return [(\"A2\", \"updated\")]\n    sheet = workbook[workbook.sheetnames[0]]\n    for cell, value in updates():\n        if sheet[cell].value is None:\n            sheet[cell] = value",
            ),
            _ => unreachable!(),
        };
        fs::write(workspace.path().join("scripts/editor.py"), editor_source).unwrap();

        let script_mount = format!(
            "{}/{}/editor.py",
            super::super::MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX,
            match case.profile {
                AgentCommandRuntimeProfile::Documents => "documents",
                AgentCommandRuntimeProfile::Spreadsheets => "spreadsheets",
                _ => unreachable!(),
            }
        );
        let permissions = editor_permissions();
        let input_context = AgentFileInputExecutionContext::default();
        let inputs = prepare_agent_file_input_bindings(
            Some(workspace.path()),
            permissions,
            &input_context,
            &[
                AgentFileInputSpec {
                    mount_path: script_mount.clone(),
                    source: AgentFileInputRef::Workspace {
                        path: "scripts/editor.py".to_string(),
                    },
                },
                AgentFileInputSpec {
                    mount_path: case.source.to_string(),
                    source: AgentFileInputRef::Workspace {
                        path: case.source.to_string(),
                    },
                },
            ],
            None,
        )
        .unwrap();
        let (_runtime, provider) =
            create_test_artifact_runtime_with_python(real_python_component_fixture(&python));
        let runtime_binding = super::super::prepare_command_runtime_profile(
            &provider,
            case.profile,
            AgentCommandRuntimeKind::Python,
        )
        .unwrap()
        .binding;
        let office_context =
            OfficeExecutionContext::new(Some(workspace.path().to_path_buf()), permissions, None);
        let destination_binding = crate::office::prepare_managed_script_binding(
            &office_context,
            case.kind,
            OfficeManagedScriptPurpose::Edit,
            script_mount,
            Some(case.source.to_string()),
            case.destination,
        )
        .unwrap();
        let request = AgentCommandRequest {
            id: format!("real-{:?}-editor", case.profile),
            command: format!(
                "python scripts/editor.py --source {} --output {}",
                case.source, case.destination
            ),
            cwd: None,
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: Some(AgentCommandArtifactObservationRequest {
                kinds: vec![AgentCommandArtifactObservationKind::Office],
                expected_outputs: vec![case.destination.to_string()],
                additional_roots: Vec::new(),
            }),
            inputs,
            runtime_binding: Some(Box::new(runtime_binding)),
            managed_office_script: Some(Box::new(destination_binding)),
        };
        let office = Arc::new(RecordingPresentationOfficeEngine::for_managed_output(
            case.destination,
        ));
        let preparation = prepare_managed_command_session(
            Some(workspace.path()),
            &request,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            ManagedCommandSessionServices {
                artifact_runtime: Some(provider),
                office_engine: Some(office.clone()),
                file_inputs: Some(&input_context),
                managed_workspace: None,
            },
        )
        .unwrap();
        let ManagedCommandSessionPreparation::Ready {
            plan,
            completion_hook,
        } = preparation
        else {
            panic!("real fixed Python Editor unexpectedly failed during preparation")
        };
        let mut command = plan.build();
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let source_index = arguments
            .iter()
            .position(|argument| argument == "--source")
            .expect("fixed Editor argv has --source");
        assert_eq!(
            arguments.get(source_index + 1).map(String::as_str),
            Some(case.source),
            "Host must preserve the approved logical mountPath for the fixed wrapper"
        );
        assert!(
            !arguments[source_index + 1].starts_with('/'),
            "private snapshot paths must not replace the logical --source contract"
        );
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "real fixed Editor failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let mut result = completion_result(&plan, output.status.code());
        result.stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        result.stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        completion_hook(&mut result, Arc::new(AtomicBool::new(false)));

        assert_eq!(office.calls(), 1);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(workspace.path().join(case.destination).is_file());
        assert!(result.artifact_observation.is_some());
        let preserved_source = std::process::Command::new(&python)
            .args(["-c", case.verify_source])
            .arg(&source_path)
            .output()
            .unwrap();
        assert!(
            preserved_source.status.success(),
            "source preservation verification failed: {}",
            String::from_utf8_lossy(&preserved_source.stderr)
        );
        let verified = std::process::Command::new(&python)
            .args(["-c", case.verify_output])
            .arg(workspace.path().join(case.destination))
            .output()
            .unwrap();
        assert!(
            verified.status.success(),
            "published output verification failed: {}",
            String::from_utf8_lossy(&verified.stderr)
        );

        fs::write(
            workspace.path().join("scripts/builder.py"),
            case.builder_template,
        )
        .unwrap();
        let profile_name = match case.profile {
            AgentCommandRuntimeProfile::Documents => "documents",
            AgentCommandRuntimeProfile::Spreadsheets => "spreadsheets",
            _ => unreachable!(),
        };
        let builder_mount = format!(
            "{}/{profile_name}/builder.py",
            super::super::MANAGED_OFFICE_SCRIPT_RESERVED_MOUNT_PREFIX,
        );
        let builder_inputs = prepare_agent_file_input_bindings(
            Some(workspace.path()),
            permissions,
            &input_context,
            &[AgentFileInputSpec {
                mount_path: builder_mount.clone(),
                source: AgentFileInputRef::Workspace {
                    path: "scripts/builder.py".to_string(),
                },
            }],
            None,
        )
        .unwrap();
        let (_builder_runtime, builder_provider) =
            create_test_artifact_runtime_with_python(real_python_component_fixture(&python));
        let builder_runtime_binding = super::super::prepare_command_runtime_profile(
            &builder_provider,
            case.profile,
            AgentCommandRuntimeKind::Python,
        )
        .unwrap()
        .binding;
        let builder_destination_binding = crate::office::prepare_managed_script_binding(
            &office_context,
            case.kind,
            OfficeManagedScriptPurpose::Create,
            builder_mount,
            None,
            case.builder_destination,
        )
        .unwrap();
        let builder_request = AgentCommandRequest {
            id: format!("real-{:?}-builder", case.profile),
            command: format!(
                "python scripts/builder.py --output {}",
                case.builder_destination
            ),
            cwd: None,
            timeout_ms: Some(30_000),
            approval_status: AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: Some(AgentCommandArtifactObservationRequest {
                kinds: vec![AgentCommandArtifactObservationKind::Office],
                expected_outputs: vec![case.builder_destination.to_string()],
                additional_roots: Vec::new(),
            }),
            inputs: builder_inputs,
            runtime_binding: Some(Box::new(builder_runtime_binding)),
            managed_office_script: Some(Box::new(builder_destination_binding)),
        };
        let builder_office = Arc::new(RecordingPresentationOfficeEngine::for_managed_output(
            case.builder_destination,
        ));
        let builder_preparation = prepare_managed_command_session(
            Some(workspace.path()),
            &builder_request,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            ManagedCommandSessionServices {
                artifact_runtime: Some(builder_provider),
                office_engine: Some(builder_office.clone()),
                file_inputs: Some(&input_context),
                managed_workspace: None,
            },
        )
        .unwrap();
        let ManagedCommandSessionPreparation::Ready {
            plan: builder_plan,
            completion_hook: builder_completion,
        } = builder_preparation
        else {
            panic!("real fixed Python Builder unexpectedly failed during preparation")
        };
        let builder_output = builder_plan.build().output().unwrap();
        assert!(
            builder_output.status.success(),
            "real fixed Builder failed: stdout={} stderr={}",
            String::from_utf8_lossy(&builder_output.stdout),
            String::from_utf8_lossy(&builder_output.stderr)
        );
        let mut builder_result = completion_result(&builder_plan, builder_output.status.code());
        builder_completion(&mut builder_result, Arc::new(AtomicBool::new(false)));
        assert_eq!(builder_office.calls(), 1);
        assert!(builder_result.error.is_none(), "{:?}", builder_result.error);
        assert!(builder_result.artifact_observation.is_some());
        let verified_builder = std::process::Command::new(&python)
            .args(["-c", case.verify_builder])
            .arg(workspace.path().join(case.builder_destination))
            .output()
            .unwrap();
        assert!(
            verified_builder.status.success(),
            "published Builder output verification failed: {}",
            String::from_utf8_lossy(&verified_builder.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn presentation_editor_completion_applies_office_once_before_after_observation() {
    let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Success);
    let output = fixture.plan.build().output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut result = completion_result(&fixture.plan, output.status.code());

    (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

    assert_eq!(fixture.office.calls(), 1);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.error.is_none());
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
    assert!(fixture.workspace.path().join("edited.pptx").is_file());
    let observation = result
        .artifact_observation
        .as_ref()
        .expect("successful Host edit must perform the After capture");
    assert!(observation.coverage.after.roots_scanned > 0);
    assert!(observation
        .changes
        .iter()
        .any(|change| change.path == "edited.pptx"));
}
#[cfg(unix)]
#[test]
fn presentation_editor_completion_office_failure_fails_command_without_after_observation() {
    let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Failure);
    let output = fixture.plan.build().output().unwrap();
    assert!(output.status.success());
    let mut result = completion_result(&fixture.plan, output.status.code());

    (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

    assert_eq!(fixture.office.calls(), 1);
    assert_eq!(result.exit_code, Some(7));
    let error = result.error.as_deref().expect("bounded Office diagnostic");
    assert!(error.contains("invalid_target"));
    assert!(error.contains("Could not find the inspected target."));
    assert!(error.contains("Re-inspect the deck."));
    assert!(error.contains("office.fixture_failure"));
    assert!(error.contains("fixture Office failure"));
    assert!(!error.contains("/private/var/folders/secret"));
    assert!(!error.contains("presentation-edit-plan.json"));
    assert!(error.chars().count() <= 4_096);
    assert!(result.artifact_observation.is_none());
    assert!(!fixture.workspace.path().join("edited.pptx").exists());
}

#[cfg(unix)]
#[test]
fn presentation_editor_completion_plan_and_cancellation_failures_never_publish_after() {
    #[derive(Clone, Copy, Debug)]
    enum FailureCase {
        MissingPlan,
        MalformedPlan,
        OfficeCancelled,
    }

    for case in [
        FailureCase::MissingPlan,
        FailureCase::MalformedPlan,
        FailureCase::OfficeCancelled,
    ] {
        let outcome = match case {
            FailureCase::OfficeCancelled => TestOfficeOutcome::Cancelled,
            FailureCase::MissingPlan | FailureCase::MalformedPlan => TestOfficeOutcome::Success,
        };
        let fixture = prepare_editor_completion_fixture(outcome);
        let mut result = completion_result(&fixture.plan, Some(0));
        if !matches!(case, FailureCase::MissingPlan) {
            let mut command = fixture.plan.build();
            let plan_path = command
                .get_envs()
                .find_map(|(name, value)| {
                    (name == OsStr::new(super::super::PRESENTATION_EDITOR_PLAN_ENV))
                        .then_some(value)
                        .flatten()
                })
                .map(PathBuf::from)
                .expect("Editor plan path is a Host-owned launch environment value");
            let output = command.output().unwrap();
            assert!(output.status.success(), "case={case:?}");
            if matches!(case, FailureCase::MalformedPlan) {
                fs::write(plan_path, b"{malformed").unwrap();
            }
        }

        (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

        assert!(result.error.is_some(), "case={case:?}");
        assert!(result.artifact_observation.is_none(), "case={case:?}");
        assert!(
            !fixture.workspace.path().join("edited.pptx").exists(),
            "case={case:?}"
        );
        match case {
            FailureCase::OfficeCancelled => {
                assert_eq!(fixture.office.calls(), 1);
                assert!(result.cancelled);
                assert_eq!(result.exit_code, None);
            }
            FailureCase::MissingPlan | FailureCase::MalformedPlan => {
                assert_eq!(fixture.office.calls(), 0);
                assert!(!result.cancelled);
                assert_eq!(result.exit_code, Some(1));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn presentation_editor_completion_node_failure_skips_office_and_after_observation() {
    let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Success);
    let mut result = completion_result(&fixture.plan, Some(9));

    (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

    assert_eq!(fixture.office.calls(), 0);
    assert_eq!(result.exit_code, Some(9));
    assert!(result.artifact_observation.is_none());
    assert!(!fixture.workspace.path().join("edited.pptx").exists());
}
