use super::*;
use crate::file_change::FileObservationOwner;

#[test]
fn run_grant_checkpoint_requires_exact_canonical_action_and_strict_apply_patch_call() {
    let run_id = "checkpoint-validation-run";
    let (mut checkpoint, mut continuation) = restorable_checkpoint_fixture();
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let args = apply_patch_args(json!({
        "action": "apply",
        "operation": "create",
        "filePath": "report.txt",
        "content": "current\n"
    }));
    for item in &mut checkpoint.context_items {
        for call in &mut item.tool_calls {
            if call.id == pending_call_id {
                call.args = args.clone();
            }
        }
    }
    for item in &mut checkpoint.conversation_trace_items {
        if let ConversationTurnTraceItem::ToolCall {
            call_id,
            tool,
            provenance,
            operation,
            approval_status,
            ..
        } = item
        {
            if call_id == &pending_call_id {
                *tool = "apply_patch".to_string();
                *provenance = AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                };
                *operation = args.clone();
                *approval_status = AgentApprovalStatus::Approved;
            }
        }
    }
    checkpoint.pending_action_id =
        Some(crate::canonical_pending_action_id(run_id, &pending_call_id));
    checkpoint.file_change_run_grant_ref = Some(crate::file_change::FileChangeRunGrantRef {
        schema_version: crate::file_change::FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
        grant_id: "grant-checkpoint".to_string(),
        revision: 1,
        apply_patch_contract_revision: crate::file_change::APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
            .to_string(),
    });
    continuation.call.args = args;
    assert!(restore_run_checkpoint(checkpoint.clone(), run_id, &continuation).is_ok());

    let mut wrong_action = checkpoint.clone();
    wrong_action.pending_action_id = Some(crate::canonical_pending_action_id(
        run_id,
        "another-call-in-the-same-run",
    ));
    assert!(restore_run_checkpoint(wrong_action, run_id, &continuation).is_err());

    let mut wrong_trace_tool = checkpoint;
    for item in &mut wrong_trace_tool.conversation_trace_items {
        if let ConversationTurnTraceItem::ToolCall { call_id, tool, .. } = item {
            if call_id == &pending_call_id {
                *tool = "read_file".to_string();
            }
        }
    }
    assert!(restore_run_checkpoint(wrong_trace_tool, run_id, &continuation).is_err());
}

#[test]
fn approval_restore_issues_a_verified_successor_observation_before_resuming_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    let target = workspace.join("report.txt");
    std::fs::write(&target, "after\n").unwrap();
    let revision = crate::content_revision(b"after\n");
    let run_id = "checkpoint-validation-run";
    let conversation_id = "checkpoint-successor-restore";
    let (mut checkpoint, mut continuation) = restorable_checkpoint_fixture();
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let args = apply_patch_args(json!({
        "action":"apply",
        "operation":"create",
        "filePath":"report.txt",
        "content":"after\n"
    }));
    for item in &mut checkpoint.context_items {
        for call in &mut item.tool_calls {
            if call.id == pending_call_id {
                call.args = args.clone();
            }
        }
    }
    for item in &mut checkpoint.conversation_trace_items {
        if let ConversationTurnTraceItem::ToolCall {
            call_id,
            tool,
            provenance,
            operation,
            approval_status,
            ..
        } = item
        {
            if call_id == &pending_call_id {
                *tool = "apply_patch".to_string();
                *provenance = AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                };
                *operation = args.clone();
                *approval_status = AgentApprovalStatus::Approved;
            }
        }
    }
    checkpoint.pending_action_id =
        Some(crate::canonical_pending_action_id(run_id, &pending_call_id));
    checkpoint.file_change_run_grant_ref = Some(crate::file_change::FileChangeRunGrantRef {
        schema_version: crate::file_change::FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
        grant_id: "grant-successor-restore".to_string(),
        revision: 1,
        apply_patch_contract_revision: crate::file_change::APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
            .to_string(),
    });
    checkpoint.run_context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            folders: Vec::new(),
            project_id: None,
            display_name: None,
            root_path: Some(workspace.display().to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            read: crate::protocol::AgentReadPermission::WorkspaceOnly,
            write: crate::protocol::AgentWritePermission::WorkspaceOnly,
            ..crate::protocol::AgentPermissions::default()
        },
    });
    continuation.call.args = args;
    let terminal = crate::protocol::AgentFileChangeResult {
        schema_version: crate::protocol::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::protocol::AgentFileChangeResultStatus::Applied,
        outcome: crate::protocol::AgentFileChangeOutcome::Applied,
        transaction_id: "file-change-direct-v1:successor-restore".to_string(),
        operation: crate::protocol::AgentFileChangeOperation::Create,
        update_strategy: None,
        file_path: "report.txt".to_string(),
        additions: 1,
        deletions: 0,
        line_count: 1,
        byte_count: 6,
        revision: Some(revision),
        error_code: None,
        error: None,
        message: Some("文件变更已应用。".to_string()),
    };
    terminal.validate().unwrap();
    continuation.result = crate::protocol::AgentToolResult {
        exact_archive_file: None,
        call_id: pending_call_id,
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(serde_json::to_value(terminal).unwrap()),
        error: None,
    };

    let restored = restore_run_checkpoint(checkpoint, run_id, &continuation).unwrap();
    let observation: serde_json::Value =
        serde_json::from_str(restored.context.to_messages().last().unwrap().content()).unwrap();
    let observation_id = observation["observationId"].as_str().unwrap();
    assert_eq!(observation["fileChangeTarget"]["state"], "existing");
    assert!(restored
        .file_observations
        .validate(observation_id, conversation_id, run_id, &target)
        .is_ok());
}

#[test]
fn direct_update_approval_restore_renews_the_exact_pending_observation_id() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    let target = workspace.join("report.txt");
    std::fs::write(&target, "before\n").unwrap();
    let run_id = "checkpoint-validation-run";
    let conversation_id = "checkpoint-update-successor-restore";
    let read_call_id = canonical_test_call_id(7, "provider-read-before-update");
    let registry = FileObservationRegistry::default();
    let observation = registry
        .issue_existing(
            FileObservationOwner::new(&read_call_id, conversation_id, run_id),
            &target,
            &crate::content_revision(b"before\n"),
            &std::fs::metadata(&target).unwrap(),
            &std::fs::metadata(&workspace).unwrap(),
        )
        .unwrap();
    let predecessor_id = observation.id().to_string();

    let (mut checkpoint, mut continuation) = restorable_checkpoint_fixture();
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let args = apply_patch_args(json!({
        "action":"apply",
        "operation":"update",
        "filePath":"report.txt",
        "observationId":predecessor_id,
        "content":"after\n"
    }));
    checkpoint.context_items.splice(
        0..0,
        [
            AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "assistant".to_string(),
                content: String::new(),
                images: Vec::new(),
                tool_call_id: None,
                tool_calls: vec![AgentContextCheckpointToolCall {
                    id: read_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "report.txt" }),
                    provider_identity: AgentProviderToolCallIdentity {
                        provider_tool_index: 7,
                        provider_call_id: "provider-read-before-update".to_string(),
                        runtime_call_id: read_call_id.clone(),
                    },
                }],
                is_error: false,
                sources: vec!["model_response".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: Some(crate::AgentContextCheckpointGroup {
                    id: "read-before-update".to_string(),
                    kind: "tool_exchange".to_string(),
                }),
                origin: None,
            },
            AgentContextCheckpointItem {
                context_image_refs: Vec::new(),
                role: "tool".to_string(),
                content: json!({
                    "path": "report.txt",
                    "exists": true,
                    "revision": crate::content_revision(b"before\n"),
                    "observationId": predecessor_id,
                    "content": "before\n"
                })
                .to_string(),
                images: Vec::new(),
                tool_call_id: Some(read_call_id),
                tool_calls: Vec::new(),
                is_error: false,
                sources: vec!["tool_result".to_string()],
                scope: "run".to_string(),
                retention: "retained".to_string(),
                request_order: None,
                group: Some(crate::AgentContextCheckpointGroup {
                    id: "read-before-update".to_string(),
                    kind: "tool_exchange".to_string(),
                }),
                origin: None,
            },
        ],
    );
    for item in &mut checkpoint.context_items {
        for call in &mut item.tool_calls {
            if call.id == pending_call_id {
                call.args = args.clone();
            }
        }
    }
    for item in &mut checkpoint.conversation_trace_items {
        if let ConversationTurnTraceItem::ToolCall {
            call_id,
            tool,
            provenance,
            operation,
            approval_status,
            ..
        } = item
        {
            if call_id == &pending_call_id {
                *tool = "apply_patch".to_string();
                *provenance = AgentToolIdentity::Builtin {
                    tool_name: "apply_patch".to_string(),
                };
                *operation = args.clone();
                *approval_status = AgentApprovalStatus::Approved;
            }
        }
    }
    checkpoint.pending_action_id =
        Some(crate::canonical_pending_action_id(run_id, &pending_call_id));
    checkpoint.file_change_run_grant_ref = Some(crate::file_change::FileChangeRunGrantRef {
        schema_version: crate::file_change::FILE_CHANGE_RUN_GRANT_SCHEMA_VERSION,
        grant_id: "grant-update-successor-restore".to_string(),
        revision: 1,
        apply_patch_contract_revision: crate::file_change::APPLY_PATCH_RUN_GRANT_CONTRACT_REVISION
            .to_string(),
    });
    checkpoint.pending_file_observation = Some(observation.checkpoint());
    checkpoint.run_context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            folders: Vec::new(),
            project_id: None,
            display_name: None,
            root_path: Some(workspace.display().to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            read: crate::protocol::AgentReadPermission::WorkspaceOnly,
            write: crate::protocol::AgentWritePermission::WorkspaceOnly,
            ..crate::protocol::AgentPermissions::default()
        },
    });
    assert!(serde_json::to_value(&checkpoint).unwrap()["pendingFileObservation"].is_object());

    std::fs::write(&target, "after\n").unwrap();
    continuation.call.args = args;
    let revision = crate::content_revision(b"after\n");
    let terminal = crate::protocol::AgentFileChangeResult {
        schema_version: crate::protocol::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::protocol::AgentFileChangeResultStatus::Applied,
        outcome: crate::protocol::AgentFileChangeOutcome::Applied,
        transaction_id: "file-change-direct-v1:update-successor-restore".to_string(),
        operation: crate::protocol::AgentFileChangeOperation::Update,
        update_strategy: None,
        file_path: "report.txt".to_string(),
        additions: 1,
        deletions: 1,
        line_count: 1,
        byte_count: 6,
        revision: Some(revision),
        error_code: None,
        error: None,
        message: Some("文件变更已应用。".to_string()),
    };
    terminal.validate().unwrap();
    continuation.result = crate::protocol::AgentToolResult {
        exact_archive_file: None,
        call_id: pending_call_id,
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(serde_json::to_value(terminal).unwrap()),
        error: None,
    };

    let restored = restore_run_checkpoint(checkpoint, run_id, &continuation).unwrap();
    let model_result: serde_json::Value =
        serde_json::from_str(restored.context.to_messages().last().unwrap().content()).unwrap();
    assert_eq!(model_result["observationId"], predecessor_id);
    assert_eq!(
        model_result["fileChangeTarget"],
        json!({
            "filePath": "report.txt",
            "observationId": predecessor_id,
            "state": "existing"
        })
    );
    assert!(restored
        .file_observations
        .validate(&predecessor_id, conversation_id, run_id, &target)
        .is_ok());
}

#[test]
fn queued_apply_patch_checkpoint_restores_only_its_exact_unconsumed_read_observation() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("example.txt");
    std::fs::write(&target, "before\n").unwrap();
    let target = target.canonicalize().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    let target_metadata = std::fs::metadata(&target).unwrap();
    let parent_metadata = std::fs::metadata(&workspace).unwrap();
    let run_id = "checkpoint-validation-run";
    let conversation_id = "conversation-observation";
    let source_call_id = canonical_test_call_id(7, "provider-read-observation");
    let queued_call_id = canonical_test_call_id(8, "provider-queued-apply-patch");
    let registry = FileObservationRegistry::default();
    let observation = registry
        .issue_existing(
            FileObservationOwner::new(&source_call_id, conversation_id, run_id),
            &target,
            "revision-before",
            &target_metadata,
            &parent_metadata,
        )
        .unwrap();
    let extra_observation = registry
        .issue_missing(
            FileObservationOwner::new(
                &canonical_test_call_id(9, "provider-extra-read"),
                conversation_id,
                run_id,
            ),
            &workspace.join("unreferenced.txt"),
            &parent_metadata,
        )
        .unwrap();
    let run_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            folders: Vec::new(),
            project_id: None,
            display_name: Some("Observation workspace".to_string()),
            root_path: Some(workspace.display().to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            read: crate::protocol::AgentReadPermission::WorkspaceOnly,
            write: crate::protocol::AgentWritePermission::WorkspaceOnly,
            ..crate::protocol::AgentPermissions::default()
        },
    };
    let context_items = vec![
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![AgentContextCheckpointToolCall {
                id: source_call_id.clone(),
                name: "read_file".to_string(),
                args: json!({ "path": "example.txt" }),
                provider_identity: AgentProviderToolCallIdentity {
                    provider_tool_index: 7,
                    provider_call_id: "provider-read-observation".to_string(),
                    runtime_call_id: source_call_id.clone(),
                },
            }],
            is_error: false,
            sources: vec!["model_response".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "tool".to_string(),
            content: json!({
                "path": "example.txt",
                "exists": true,
                "revision": "revision-before",
                "observationId": observation.id(),
                "content": "before\n"
            })
            .to_string(),
            images: Vec::new(),
            tool_call_id: Some(source_call_id.clone()),
            tool_calls: Vec::new(),
            is_error: false,
            sources: vec!["tool_result".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
    ];
    let mut queued = vec![AgentQueuedToolCallCheckpoint {
        call: AgentContextCheckpointToolCall {
            id: queued_call_id.clone(),
            name: "apply_patch".to_string(),
            args: apply_patch_args(json!({
                "action": "apply",
                "operation": "update",
                "filePath": "example.txt",
                "observationId": observation.id(),
                "content": "after\n"
            })),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 8,
                provider_call_id: "provider-queued-apply-patch".to_string(),
                runtime_call_id: queued_call_id,
            },
        },
        file_observation: None,
        assistant_content: String::new(),
        group_id: "group-observation".to_string(),
        assistant_turn_id: "assistant-turn-observation".to_string(),
        provider_tool_index: 8,
    }];

    attach_queued_file_observations(
        &mut queued,
        &registry,
        run_id,
        Some(&run_context),
        &context_items,
        None,
    )
    .unwrap();
    let frozen = queued[0]
        .file_observation
        .as_ref()
        .expect("queued apply_patch freezes its exact observation");
    assert_eq!(frozen.observation_id, observation.id());
    assert_eq!(frozen.source_tool_call_id, source_call_id);

    let restored =
        restore_queued_file_observations(&queued, run_id, Some(&run_context), &context_items, None)
            .unwrap();
    assert!(restored
        .validate(observation.id(), conversation_id, run_id, target.as_path(),)
        .is_ok());
    assert_eq!(
        restored
            .validate(
                extra_observation.id(),
                conversation_id,
                run_id,
                &workspace.join("unreferenced.txt"),
            )
            .unwrap_err()
            .code(),
        crate::file_change::FileChangeErrorCode::ObservationRequired
    );

    let mut missing = serde_json::to_value(&queued[0]).unwrap();
    missing.as_object_mut().unwrap().remove("fileObservation");
    assert!(serde_json::from_value::<AgentQueuedToolCallCheckpoint>(missing).is_err());
}

#[test]
fn queued_apply_patch_checkpoint_accepts_only_an_exact_successful_apply_patch_successor() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    let target = workspace.join("example.txt");
    std::fs::write(&target, "after\n").unwrap();
    let revision = crate::content_revision(b"after\n");
    let run_id = "checkpoint-successor-run";
    let conversation_id = "checkpoint-successor-conversation";
    let source_call_id = canonical_test_call_id(7, "provider-successful-apply");
    let queued_call_id = canonical_test_call_id(8, "provider-successor-apply");
    let registry = FileObservationRegistry::default();
    let observation = registry
        .issue_existing(
            FileObservationOwner::new(&source_call_id, conversation_id, run_id),
            &target,
            &revision,
            &std::fs::metadata(&target).unwrap(),
            &std::fs::metadata(&workspace).unwrap(),
        )
        .unwrap();
    let run_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            folders: Vec::new(),
            project_id: None,
            display_name: None,
            root_path: Some(workspace.display().to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            write: crate::protocol::AgentWritePermission::WorkspaceOnly,
            ..crate::protocol::AgentPermissions::default()
        },
    };
    let source_call = AgentContextCheckpointToolCall {
        id: source_call_id.clone(),
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action":"apply",
            "operation":"update",
            "filePath":"example.txt",
            "observationId":format!("fobs_{}", "1".repeat(32)),
            "content":"after\n"
        })),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: 7,
            provider_call_id: "provider-successful-apply".to_string(),
            runtime_call_id: source_call_id.clone(),
        },
    };
    let terminal = crate::protocol::AgentFileChangeResult {
        schema_version: crate::protocol::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::protocol::AgentFileChangeResultStatus::Applied,
        outcome: crate::protocol::AgentFileChangeOutcome::Applied,
        transaction_id: "file-change-direct-v1:checkpoint-successor".to_string(),
        operation: crate::protocol::AgentFileChangeOperation::Update,
        update_strategy: None,
        file_path: "example.txt".to_string(),
        additions: 1,
        deletions: 1,
        line_count: 1,
        byte_count: 6,
        revision: Some(revision.clone()),
        error_code: None,
        error: None,
        message: Some("文件变更已应用。".to_string()),
    };
    terminal.validate().unwrap();
    let mut terminal_result = serde_json::to_value(&terminal).unwrap();
    terminal_result["observationId"] = json!(observation.id());
    terminal_result["fileChangeTarget"] = json!({
        "filePath":"example.txt",
        "observationId":observation.id(),
        "state":"existing"
    });
    let context_items = vec![
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![source_call],
            is_error: false,
            sources: vec!["model_response".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "tool".to_string(),
            content: terminal_result.to_string(),
            images: Vec::new(),
            tool_call_id: Some(source_call_id),
            tool_calls: Vec::new(),
            is_error: false,
            sources: vec!["tool_result".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
    ];
    let mut queued = vec![AgentQueuedToolCallCheckpoint {
        call: AgentContextCheckpointToolCall {
            id: queued_call_id.clone(),
            name: "apply_patch".to_string(),
            args: apply_patch_args(json!({
                "action":"apply",
                "operation":"update",
                "filePath":"example.txt",
                "observationId":observation.id(),
                "edits":[{"kind":"append","text":"next\n"}]
            })),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 8,
                provider_call_id: "provider-successor-apply".to_string(),
                runtime_call_id: queued_call_id,
            },
        },
        file_observation: None,
        assistant_content: String::new(),
        group_id: "group-successor".to_string(),
        assistant_turn_id: "assistant-turn-successor".to_string(),
        provider_tool_index: 8,
    }];

    attach_queued_file_observations(
        &mut queued,
        &registry,
        run_id,
        Some(&run_context),
        &context_items,
        None,
    )
    .unwrap();
    assert!(restore_queued_file_observations(
        &queued,
        run_id,
        Some(&run_context),
        &context_items,
        None,
    )
    .is_ok());

    for mutate in [
        |value: &mut serde_json::Value| value["fileChangeTarget"]["state"] = json!("missing"),
        |value: &mut serde_json::Value| value["revision"] = json!("v1-tampered"),
        |value: &mut serde_json::Value| value["fileChangeTarget"]["extra"] = json!(true),
    ] {
        let mut tampered = context_items.clone();
        let mut value: serde_json::Value = serde_json::from_str(&tampered[1].content).unwrap();
        mutate(&mut value);
        tampered[1].content = value.to_string();
        assert!(restore_queued_file_observations(
            &queued,
            run_id,
            Some(&run_context),
            &tampered,
            None,
        )
        .is_err());
    }
}

#[test]
fn queued_apply_patch_checkpoint_accepts_only_an_exact_staged_commit_successor_source_chain() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().canonicalize().unwrap();
    let target = workspace.join("staged.txt");
    std::fs::write(&target, "after staged\n").unwrap();
    let revision = crate::content_revision(b"after staged\n");
    let run_id = "checkpoint-staged-successor-run";
    let conversation_id = "checkpoint-staged-successor-conversation";
    let begin_call_id = canonical_test_call_id(6, "provider-staged-begin");
    let commit_call_id = canonical_test_call_id(7, "provider-staged-commit");
    let queued_call_id = canonical_test_call_id(8, "provider-staged-successor");
    let transaction_id = "file-change-staged-v1:checkpoint-staged-successor";
    let registry = FileObservationRegistry::default();
    let observation = registry
        .issue_existing(
            FileObservationOwner::new(&commit_call_id, conversation_id, run_id),
            &target,
            &revision,
            &std::fs::metadata(&target).unwrap(),
            &std::fs::metadata(&workspace).unwrap(),
        )
        .unwrap();
    let run_context = AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: Some(crate::protocol::AgentWorkspaceContext {
            folders: Vec::new(),
            project_id: None,
            display_name: None,
            root_path: Some(workspace.display().to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            write: crate::protocol::AgentWritePermission::WorkspaceOnly,
            ..crate::protocol::AgentPermissions::default()
        },
    };
    let begin_call = AgentContextCheckpointToolCall {
        id: begin_call_id.clone(),
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action":"begin",
            "operation":"update",
            "filePath":"staged.txt",
            "observationId":format!("fobs_{}", "2".repeat(32)),
            "strategy":"modify"
        })),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: 6,
            provider_call_id: "provider-staged-begin".to_string(),
            runtime_call_id: begin_call_id.clone(),
        },
    };
    let commit_call = AgentContextCheckpointToolCall {
        id: commit_call_id.clone(),
        name: "apply_patch".to_string(),
        args: apply_patch_args(json!({
            "action":"commit",
            "transactionId":transaction_id,
            "expectedDraftRevision":1
        })),
        provider_identity: AgentProviderToolCallIdentity {
            provider_tool_index: 7,
            provider_call_id: "provider-staged-commit".to_string(),
            runtime_call_id: commit_call_id.clone(),
        },
    };
    let terminal = crate::protocol::AgentFileChangeResult {
        schema_version: crate::protocol::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::protocol::AgentFileChangeResultStatus::Applied,
        outcome: crate::protocol::AgentFileChangeOutcome::Applied,
        transaction_id: transaction_id.to_string(),
        operation: crate::protocol::AgentFileChangeOperation::Update,
        update_strategy: Some(crate::protocol::AgentFileChangeUpdateStrategy::Modify),
        file_path: "staged.txt".to_string(),
        additions: 1,
        deletions: 1,
        line_count: 1,
        byte_count: 13,
        revision: Some(revision),
        error_code: None,
        error: None,
        message: Some("文件变更已应用。".to_string()),
    };
    terminal.validate().unwrap();
    let mut terminal_result = serde_json::to_value(&terminal).unwrap();
    terminal_result["observationId"] = json!(observation.id());
    terminal_result["fileChangeTarget"] = json!({
        "filePath":"staged.txt",
        "observationId":observation.id(),
        "state":"existing"
    });
    let context_items = vec![
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![begin_call],
            is_error: false,
            sources: vec!["model_response".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "tool".to_string(),
            content: json!({
                "transactionId": transaction_id,
                "operation": "update",
                "strategy": "modify",
                "filePath": "staged.txt",
                "status": "drafting",
                "draftRevision": 0,
                "nextIndex": 0,
                "byteCount": 13,
                "lineCount": 1,
                "mutationCount": 0,
                "additions": 0,
                "deletions": 0,
                "tail": "before staged\n",
                "totalChars": 14,
                "tailStart": 0,
                "tailTruncated": false,
                "allowedNextActions": ["append", "edit", "commit", "status", "abort"],
                "requiresCommitBeforeResponse": true
            })
            .to_string(),
            images: Vec::new(),
            tool_call_id: Some(begin_call_id),
            tool_calls: Vec::new(),
            is_error: false,
            sources: vec!["tool_result".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "assistant".to_string(),
            content: String::new(),
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: vec![commit_call],
            is_error: false,
            sources: vec!["model_response".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
        AgentContextCheckpointItem {
            context_image_refs: Vec::new(),
            role: "tool".to_string(),
            content: terminal_result.to_string(),
            images: Vec::new(),
            tool_call_id: Some(commit_call_id),
            tool_calls: Vec::new(),
            is_error: false,
            sources: vec!["tool_result".to_string()],
            scope: "run".to_string(),
            retention: "retained".to_string(),
            request_order: None,
            group: None,
            origin: None,
        },
    ];
    let mut queued = vec![AgentQueuedToolCallCheckpoint {
        call: AgentContextCheckpointToolCall {
            id: queued_call_id.clone(),
            name: "apply_patch".to_string(),
            args: apply_patch_args(json!({
                "action":"apply",
                "operation":"update",
                "filePath":"staged.txt",
                "observationId":observation.id(),
                "edits":[{"kind":"append","text":"next\n"}]
            })),
            provider_identity: AgentProviderToolCallIdentity {
                provider_tool_index: 8,
                provider_call_id: "provider-staged-successor".to_string(),
                runtime_call_id: queued_call_id,
            },
        },
        file_observation: None,
        assistant_content: String::new(),
        group_id: "group-staged-successor".to_string(),
        assistant_turn_id: "assistant-turn-staged-successor".to_string(),
        provider_tool_index: 8,
    }];

    attach_queued_file_observations(
        &mut queued,
        &registry,
        run_id,
        Some(&run_context),
        &context_items,
        None,
    )
    .unwrap();
    assert!(restore_queued_file_observations(
        &queued,
        run_id,
        Some(&run_context),
        &context_items,
        None
    )
    .is_ok());

    let assert_rejected = |tampered: &[AgentContextCheckpointItem]| {
        assert!(restore_queued_file_observations(
            &queued,
            run_id,
            Some(&run_context),
            tampered,
            None,
        )
        .is_err());
    };

    let mut wrong_begin_path = context_items.clone();
    wrong_begin_path[0].tool_calls[0].args["request"]["filePath"] = json!("other.txt");
    assert_rejected(&wrong_begin_path);

    let mut wrong_transaction = context_items.clone();
    let mut begin_result: serde_json::Value =
        serde_json::from_str(&wrong_transaction[1].content).unwrap();
    begin_result["transactionId"] = json!("file-change-staged-v1:tampered");
    wrong_transaction[1].content = begin_result.to_string();
    assert_rejected(&wrong_transaction);

    let mut wrong_operation = context_items.clone();
    wrong_operation[0].tool_calls[0].args["request"]["operation"] = json!("create");
    wrong_operation[0].tool_calls[0].args["request"]
        .as_object_mut()
        .unwrap()
        .remove("strategy");
    let mut begin_result: serde_json::Value =
        serde_json::from_str(&wrong_operation[1].content).unwrap();
    begin_result["operation"] = json!("create");
    begin_result["strategy"] = serde_json::Value::Null;
    wrong_operation[1].content = begin_result.to_string();
    assert_rejected(&wrong_operation);

    let mut wrong_strategy = context_items.clone();
    wrong_strategy[0].tool_calls[0].args["request"]["strategy"] = json!("rewrite");
    assert_rejected(&wrong_strategy);

    let mut wrong_terminal_path = context_items.clone();
    let mut terminal_result: serde_json::Value =
        serde_json::from_str(&wrong_terminal_path[3].content).unwrap();
    terminal_result["filePath"] = json!("other.txt");
    wrong_terminal_path[3].content = terminal_result.to_string();
    assert_rejected(&wrong_terminal_path);

    for mutate in [
        |value: &mut serde_json::Value| value["status"] = json!("ready"),
        |value: &mut serde_json::Value| value["extra"] = json!(true),
    ] {
        let mut malformed_begin_result = context_items.clone();
        let mut begin_result: serde_json::Value =
            serde_json::from_str(&malformed_begin_result[1].content).unwrap();
        mutate(&mut begin_result);
        malformed_begin_result[1].content = begin_result.to_string();
        assert_rejected(&malformed_begin_result);
    }

    let mut late_begin_result = context_items.clone();
    late_begin_result.swap(1, 2);
    assert_rejected(&late_begin_result);
}
