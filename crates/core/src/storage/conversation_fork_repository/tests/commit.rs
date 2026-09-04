#[test]
fn fork_remaps_current_apply_patch_staged_transaction_and_read_observation() {
    use crate::file_change::{
        FileChangeBase, FileChangeDirectBinding, FileChangeMutation, FileChangeMutationReceipt,
        FileChangeOperation, FileChangeOutcome, FileChangePlanRequest, FileChangePlanner,
        FileChangeProposal, FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FileObservationState, FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        FILE_CHANGE_SCHEMA_VERSION,
    };

    const SOURCE_TRANSACTION_ID: &str = "file-change-apply-source";
    const READ_CALL_ID: &str = "call-apply-read";
    const BEGIN_CALL_ID: &str = "call-apply-begin";
    const APPEND_CALL_ID: &str = "call-apply-append";
    const COMMIT_CALL_ID: &str = "call-apply-commit";

    let fixture = tempfile::tempdir().unwrap();
    let canonical_target = fixture.path().join("notes.md");
    std::fs::write(&canonical_target, "before\n").unwrap();
    let base_revision = crate::content_revision(b"before\n");

    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let (observation_id, observation_json) = apply_patch_observation(
        &source.id,
        "run-source-1",
        READ_CALL_ID,
        &canonical_target,
        &base_revision,
    );
    let source_observation: FileObservationCheckpoint =
        serde_json::from_str(&observation_json).unwrap();

    let read_args = json!({ "path": "notes.md" });
    let begin_args = apply_patch_args(json!({
        "action": "begin",
        "operation": "update",
        "filePath": "notes.md",
        "observationId": observation_id,
        "strategy": "rewrite",
    }));
    let append_args = apply_patch_args(json!({
        "action": "append",
        "transactionId": SOURCE_TRANSACTION_ID,
        "index": 0,
        "expectedDraftRevision": 0,
        "content": "after\n",
    }));
    let commit_args = apply_patch_args(json!({
        "action": "commit",
        "transactionId": SOURCE_TRANSACTION_ID,
        "expectedDraftRevision": 1,
        "summary": "replace notes",
    }));

    for (index, message) in source
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .enumerate()
    {
        let items = if message.id == "assistant-b" {
            let mut items = staged_tool_exchange(0, READ_CALL_ID, "read_file", read_args.clone());
            if let ConversationTurnTraceItem::ToolResult { observation, .. } = &mut items[1] {
                *observation = json!({
                    "filePath": "notes.md",
                    "revision": base_revision,
                    "observationId": observation_id,
                });
            }
            items.extend(staged_tool_exchange(
                2,
                BEGIN_CALL_ID,
                "apply_patch",
                begin_args.clone(),
            ));
            items.extend(staged_tool_exchange(
                4,
                APPEND_CALL_ID,
                "apply_patch",
                append_args.clone(),
            ));
            let mut commit =
                staged_tool_exchange(6, COMMIT_CALL_ID, "apply_patch", commit_args.clone());
            if let ConversationTurnTraceItem::ToolCall {
                approval_status, ..
            } = &mut commit[0]
            {
                *approval_status = AgentApprovalStatus::Required;
            }
            if let ConversationTurnTraceItem::ToolResult {
                approval_status, ..
            } = &mut commit[1]
            {
                *approval_status = AgentApprovalStatus::Approved;
            }
            if let ConversationTurnTraceItem::ToolCall { operation, .. } = &mut commit[0] {
                *operation =
                    crate::file_change_support::apply_patch_trace_operation(&commit_args).unwrap();
            }
            items.extend(commit);
            items
        } else {
            vec![ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: format!("narration {index}"),
                truncated: false,
            }]
        };
        conversation_trace_repository::replace_trace(
            &mut connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: format!("run-source-{index}"),
                conversation_id: source.id.clone(),
                assistant_message_id: message.id.clone(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items,
            },
            message.created_at,
            message.created_at + 1,
        )
        .unwrap();
    }

    file_change_repository::insert_file_change(
        &connection,
        &AgentFileChangeRecord {
            schema_version: 1,
            id: SOURCE_TRANSACTION_ID.to_string(),
            conversation_id: source.id.clone(),
            project_id: None,
            run_id: "run-source-1".to_string(),
            source_tool_name: "apply_patch".to_string(),
            source_tool_call_id: BEGIN_CALL_ID.to_string(),
            source_tool_arguments_digest: crate::file_change::proposal_digest(&begin_args).unwrap(),
            permission_revision: "permission-1".to_string(),
            tool_set_revision: "tool-set-1".to_string(),
            provider_wire_revision: "provider-protocol-v1".to_string(),
            observation_id: observation_id.clone(),
            observation_json,
            file_path: "notes.md".to_string(),
            operation: "update".to_string(),
            strategy: Some("rewrite".to_string()),
            status: "applied".to_string(),
            base_revision: Some(base_revision.clone()),
            base_content: "before\n".to_string(),
            content: "after\n".to_string(),
            draft_revision: 1,
            next_mutation_index: 1,
            additions: 1,
            deletions: 1,
            line_count: 1,
            byte_count: 6,
            mutation_count: 1,
            stats_final: true,
            summary: Some("replace notes".to_string()),
            final_action_id: Some(COMMIT_CALL_ID.to_string()),
            final_action_arguments_digest: Some(
                crate::file_change::proposal_digest(&commit_args).unwrap(),
            ),
            final_permission_revision: Some("permission-1".to_string()),
            final_tool_set_revision: Some("tool-set-1".to_string()),
            final_provider_wire_revision: Some("provider-protocol-v1".to_string()),
            created_at: 35,
            updated_at: 40,
            expires_at: i64::MAX,
        },
    )
    .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_chunks (
                 transaction_id, mutation_index, content_digest, byte_count, created_at
             ) VALUES (?1, 0, ?2, 6, 39)",
            params![
                SOURCE_TRANSACTION_ID,
                crate::file_change::content_digest(b"after\n")
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_file_change_operations (
                 transaction_id, mutation_index, source_tool_call_id,
                 source_tool_arguments_digest, action, payload_digest,
                 draft_revision, receipt_json, created_at
             ) VALUES (?1, 0, ?2, ?3, 'append', ?4, 1, ?5, 39)",
            params![
                SOURCE_TRANSACTION_ID,
                APPEND_CALL_ID,
                crate::file_change::proposal_digest(&append_args).unwrap(),
                crate::file_change::content_digest(b"after\n"),
                staged_mutation_receipt(SOURCE_TRANSACTION_ID, 0),
            ],
        )
        .unwrap();

    let frozen_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: "notes.md",
            base: FileChangeBase::Existing {
                content: "before\n",
                revision: &base_revision,
            },
            mutation: FileChangeMutation::Complete("after\n".to_string()),
        })
        .unwrap();
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction: FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "notes.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            created_at: 35,
            updated_at: 40,
        },
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: COMMIT_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "notes.md".to_string(),
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            diff_digest: frozen_plan.diff_digest.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
        },
        observation_id: observation_id.clone(),
        observation: source_observation.clone(),
        source_tool_name: "apply_patch".to_string(),
        source_call_id: COMMIT_CALL_ID.to_string(),
        source_args_digest: crate::file_change::proposal_digest(&commit_args).unwrap(),
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&commit_args)
            .unwrap(),
        staged_transaction_id: Some(SOURCE_TRANSACTION_ID.to_string()),
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-1".to_string(),
        staged_transaction_revision: Some(1),
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        base_content: Some("before\n".to_string()),
        target_content: Some("after\n".to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
    };
    execution.validate().unwrap();
    let audit_action = AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: COMMIT_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: crate::AgentFileChangeOperation::Update,
            update_strategy: Some(crate::AgentFileChangeUpdateStrategy::Rewrite),
            file_path: "notes.md".to_string(),
            inline_diff: None,
            base_revision: Some(base_revision.clone()),
            summary: Some("replace notes".to_string()),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
            line_count: 1,
            byte_count: 6,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        },
    };
    if let AgentProposedAction::FileChange { file_change } = &audit_action {
        file_change.validate().unwrap();
    }
    let source_audit_id = crate::canonical_pending_action_id("run-source-1", COMMIT_CALL_ID);
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(
            &connection,
            &AgentActionAuditRecord {
                action_id: source_audit_id,
                run_id: "run-source-1".to_string(),
                conversation_id: Some(source.id.clone()),
                assistant_message_id: Some("assistant-b".to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: serde_json::to_string(&audit_action).unwrap(),
                file_change_result_json: None,
                command_result_json: None,
                tool_result_json: None,
                error: None,
                created_at: 35,
                decided_at: Some(39),
                completed_at: Some(40),
                effective_permissions_json: Some("{}".to_string()),
                path_scope: Some(canonical_target.to_string_lossy().into_owned()),
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            },
        )
        .unwrap()
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-current-apply-patch-staged",
        &source.id,
        "assistant-c",
        100,
    )
    .unwrap();
    assert_eq!(plan.file_changes.len(), 1);
    assert_eq!(plan.action_audits.len(), 1);
    let target_run_id = plan.run_id_map["run-source-1"].clone();
    let target_transaction_id = plan.file_changes[0].id.clone();
    assert_ne!(target_transaction_id, SOURCE_TRANSACTION_ID);
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_change =
        file_change_repository::get_file_change(&connection, &target_transaction_id)
            .unwrap()
            .unwrap();
    assert_eq!(target_change.conversation_id, plan.target.id);
    assert_eq!(target_change.run_id, target_run_id);
    assert_eq!(target_change.source_tool_name, "apply_patch");
    assert_ne!(target_change.source_tool_call_id, BEGIN_CALL_ID);
    assert_ne!(
        target_change.final_action_id.as_deref(),
        Some(COMMIT_CALL_ID)
    );
    assert_ne!(target_change.observation_id, observation_id);

    let target_observation: FileObservationCheckpoint =
        serde_json::from_str(&target_change.observation_json).unwrap();
    assert_eq!(
        target_observation.observation_id,
        target_change.observation_id
    );
    assert_eq!(target_observation.conversation_id, plan.target.id);
    assert_eq!(target_observation.run_id, target_run_id);
    assert_ne!(target_observation.source_tool_call_id, READ_CALL_ID);
    assert_eq!(target_observation.state, source_observation.state);
    assert_eq!(
        target_observation.parent_directory_identity,
        source_observation.parent_directory_identity
    );
    target_observation
        .validate_frozen_binding(&plan.target.id, &target_run_id, &canonical_target)
        .unwrap();

    let target_history = file_change_repository::load_file_change_history_snapshot(
        &connection,
        &target_transaction_id,
        None,
    )
    .unwrap();
    assert_eq!(target_history.chunks.len(), 1);
    assert_eq!(target_history.operations.len(), 1);
    assert_eq!(
        target_history.chunks[0].transaction_id,
        target_transaction_id
    );
    assert_eq!(
        target_history.chunks[0].content_digest,
        crate::file_change::content_digest(b"after\n")
    );
    let target_operation = &target_history.operations[0];
    assert_eq!(target_operation.transaction_id, target_transaction_id);
    assert_ne!(target_operation.source_tool_call_id, APPEND_CALL_ID);
    let receipt: FileChangeMutationReceipt =
        serde_json::from_str(&target_operation.receipt_json).unwrap();
    receipt.validate().unwrap();
    assert_eq!(receipt.transaction_id, target_transaction_id);

    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-b"],
    )
    .unwrap()
    .unwrap();
    assert_eq!(target_trace.run_id, target_run_id);
    let calls = target_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } => Some((call_id, tool, operation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let target_read = calls
        .iter()
        .find(|(_, tool, _)| tool.as_str() == "read_file")
        .unwrap();
    let target_begin = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "begin")
        .unwrap();
    let target_append = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "append")
        .unwrap();
    let target_commit = calls
        .iter()
        .find(|(_, _, operation)| operation["request"]["action"] == "commit")
        .unwrap();
    assert_eq!(
        target_read.0.as_str(),
        target_observation.source_tool_call_id
    );
    assert_eq!(target_begin.0.as_str(), target_change.source_tool_call_id);
    assert_eq!(
        target_begin.2["request"]["observationId"],
        target_change.observation_id
    );
    assert_eq!(
        target_append.0.as_str(),
        target_operation.source_tool_call_id
    );
    assert_eq!(
        target_append.2["request"]["transactionId"],
        target_transaction_id
    );
    assert_eq!(
        crate::file_change::proposal_digest(target_append.2).unwrap(),
        target_operation.source_tool_arguments_digest
    );
    assert_eq!(
        target_commit.0.as_str(),
        target_change.final_action_id.as_deref().unwrap()
    );
    assert_eq!(
        target_commit.2["request"]["transactionId"],
        target_transaction_id
    );
    let target_read_result = target_trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id,
                tool,
                observation,
                ..
            } if tool == "read_file" => Some((call_id, observation)),
            _ => None,
        })
        .unwrap();
    assert_eq!(target_read_result.0, target_read.0);
    assert_eq!(
        target_read_result.1["observationId"],
        target_change.observation_id
    );

    assert!(file_change_repository::get_file_change_for_owner(
        &connection,
        SOURCE_TRANSACTION_ID,
        &plan.target.id,
        None,
        &target_run_id,
        "apply_patch",
    )
    .unwrap()
    .is_none());
    assert!(file_change_repository::get_file_change_for_owner(
        &connection,
        &target_transaction_id,
        &plan.target.id,
        None,
        &target_run_id,
        "apply_patch",
    )
    .unwrap()
    .is_some());
    assert!(!serde_json::to_string(&target_trace)
        .unwrap()
        .contains(SOURCE_TRANSACTION_ID));
    assert!(matches!(
        target_observation.state,
        FileObservationState::Existing { .. }
    ));

    let target_commit_call_id = plan.id_replacements[COMMIT_CALL_ID].clone();
    let target_audit_id =
        crate::canonical_pending_action_id(&target_run_id, &target_commit_call_id);
    let target_audit =
        agent_action_audit_repository::load_action_audit_record(&connection, &target_audit_id)
            .unwrap()
            .unwrap();
    assert_eq!(
        target_audit.conversation_id.as_deref(),
        Some(plan.target.id.as_str())
    );
    assert_eq!(
        target_audit.assistant_message_id.as_deref(),
        Some(plan.message_id_map["assistant-b"].as_str())
    );
    let target_audit_action: AgentProposedAction =
        serde_json::from_str(&target_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: target_audit_change,
    } = target_audit_action
    else {
        unreachable!()
    };
    assert_eq!(target_audit_change.id, target_commit_call_id);
    assert_eq!(target_audit_change.transaction_id, target_transaction_id);
    assert_eq!(
        target_audit_change.execution.conversation_id,
        plan.target.id
    );
    assert_eq!(target_audit_change.execution.run_id, target_run_id);
    assert_eq!(
        target_audit_change.execution.base_content.as_deref(),
        Some("before\n")
    );
    assert_eq!(
        target_audit_change.execution.target_content.as_deref(),
        Some("after\n")
    );
    assert_eq!(
        crate::file_change::FileChangePlan::from_binding(&target_audit_change.execution)
            .unwrap()
            .diff,
        frozen_plan.diff
    );

    let recursive = build_assistant_reply_fork_plan(
        &connection,
        "fork-current-apply-patch-staged-recursive",
        &plan.target.id,
        &plan.message_id_map["assistant-c"],
        120,
    )
    .unwrap();
    assert_eq!(recursive.action_audits.len(), 1);
    commit_fork_plan(&mut connection, &recursive).unwrap();
    let recursive_run_id = recursive.run_id_map[&target_run_id].clone();
    let recursive_call_id = recursive.id_replacements[&target_commit_call_id].clone();
    let recursive_audit = agent_action_audit_repository::load_action_audit_record(
        &connection,
        &crate::canonical_pending_action_id(&recursive_run_id, &recursive_call_id),
    )
    .unwrap()
    .unwrap();
    let recursive_action: AgentProposedAction =
        serde_json::from_str(&recursive_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: recursive_change,
    } = recursive_action
    else {
        unreachable!()
    };
    assert_eq!(recursive_change.id, recursive_call_id);
    assert_ne!(recursive_change.transaction_id, target_transaction_id);
    assert_eq!(
        recursive_change.execution.conversation_id,
        recursive.target.id
    );
    assert_eq!(recursive_change.execution.run_id, recursive_run_id);
    assert_eq!(
        recursive_change.execution.base_content.as_deref(),
        Some("before\n")
    );
    assert_eq!(
        recursive_change.execution.target_content.as_deref(),
        Some("after\n")
    );
}

#[test]
fn fork_remaps_completed_direct_apply_patch_audit_without_a_staged_transaction() {
    use crate::file_change::{
        FileChangeBase, FileChangeDirectBinding, FileChangeMutation, FileChangeOperation,
        FileChangeOutcome, FileChangePlanRequest, FileChangePlanner, FileChangeProposal,
        FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
    };

    const READ_CALL_ID: &str = "call-direct-read";
    const APPLY_CALL_ID: &str = "call-direct-apply";
    const SOURCE_TRANSACTION_ID: &str = "file-change-direct-source";

    let fixture = tempfile::tempdir().unwrap();
    let canonical_target = fixture.path().join("direct.md");
    std::fs::write(&canonical_target, "before\n").unwrap();
    let base_revision = crate::content_revision(b"before\n");
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let (observation_id, observation_json) = apply_patch_observation(
        &source.id,
        "run-source-0",
        READ_CALL_ID,
        &canonical_target,
        &base_revision,
    );
    let observation: FileObservationCheckpoint = serde_json::from_str(&observation_json).unwrap();
    let read_args = json!({ "path": "direct.md" });
    let apply_args = apply_patch_args(json!({
        "action": "apply",
        "operation": "update",
        "filePath": "direct.md",
        "observationId": observation_id,
        "content": "after\n",
    }));
    let trace_apply_args =
        crate::file_change_support::apply_patch_trace_operation(&apply_args).unwrap();
    let mut items = staged_tool_exchange(0, READ_CALL_ID, "read_file", read_args);
    if let ConversationTurnTraceItem::ToolResult {
        observation: result,
        ..
    } = &mut items[1]
    {
        *result = json!({
            "filePath": "direct.md",
            "revision": base_revision,
            "observationId": observation_id,
        });
    }
    let mut apply = staged_tool_exchange(2, APPLY_CALL_ID, "apply_patch", trace_apply_args);
    if let ConversationTurnTraceItem::ToolCall {
        approval_status, ..
    } = &mut apply[0]
    {
        *approval_status = AgentApprovalStatus::Required;
    }
    if let ConversationTurnTraceItem::ToolResult {
        approval_status,
        observation,
        ..
    } = &mut apply[1]
    {
        *approval_status = AgentApprovalStatus::Approved;
        *observation = json!({
            "schemaVersion": 1,
            "status": "applied",
            "outcome": "applied",
            "transactionId": SOURCE_TRANSACTION_ID,
            "operation": "update",
            "updateStrategy": null,
            "filePath": "direct.md",
            "additions": 1,
            "deletions": 1,
            "lineCount": 1,
            "byteCount": 6,
            "revision": crate::content_revision(b"after\n"),
            "errorCode": null,
            "error": null,
            "message": null,
        });
    }
    items.extend(apply);
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-0".to_string(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: true,
            items,
        },
        20,
        21,
    )
    .unwrap();

    let frozen_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: "direct.md",
            base: FileChangeBase::Existing {
                content: "before\n",
                revision: &base_revision,
            },
            mutation: FileChangeMutation::Complete("after\n".to_string()),
        })
        .unwrap();
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction: FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            created_at: 20,
            updated_at: 20,
        },
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            diff_digest: frozen_plan.diff_digest.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
        },
        observation_id: observation_id.clone(),
        observation,
        source_tool_name: "apply_patch".to_string(),
        source_call_id: APPLY_CALL_ID.to_string(),
        source_args_digest: crate::file_change::proposal_digest(&apply_args).unwrap(),
        trace_args_digest: crate::file_change_support::apply_patch_trace_args_digest(&apply_args)
            .unwrap(),
        staged_transaction_id: None,
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-0".to_string(),
        staged_transaction_revision: None,
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        base_content: Some("before\n".to_string()),
        target_content: Some("after\n".to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
    };
    execution.validate().unwrap();
    let source_args_digest = execution.source_args_digest.clone();
    let inline_patch = frozen_plan.diff.clone();
    let action = AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: crate::AgentFileChangeOperation::Update,
            update_strategy: None,
            file_path: "direct.md".to_string(),
            inline_diff: Some(crate::AgentGitDiffSnapshot {
                patch: inline_patch.clone(),
                truncated: false,
            }),
            base_revision: Some(base_revision),
            summary: None,
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
            line_count: 1,
            byte_count: 6,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        },
    };
    let terminal_result = crate::AgentFileChangeResult {
        schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::AgentFileChangeResultStatus::Applied,
        outcome: crate::AgentFileChangeOutcome::Applied,
        transaction_id: SOURCE_TRANSACTION_ID.to_string(),
        operation: crate::AgentFileChangeOperation::Update,
        update_strategy: None,
        file_path: "direct.md".to_string(),
        additions: frozen_plan.additions,
        deletions: frozen_plan.deletions,
        line_count: 1,
        byte_count: 6,
        revision: Some(crate::content_revision(b"after\n")),
        error_code: None,
        error: None,
        message: None,
    };
    terminal_result.validate().unwrap();
    let terminal_tool_result = AgentToolResult {
        call_id: APPLY_CALL_ID.to_string(),
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(serde_json::to_value(&terminal_result).unwrap()),
        error: None,
        exact_archive_file: None,
    };
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(
            &connection,
            &AgentActionAuditRecord {
                action_id: crate::canonical_pending_action_id("run-source-0", APPLY_CALL_ID),
                run_id: "run-source-0".to_string(),
                conversation_id: Some(source.id.clone()),
                assistant_message_id: Some("assistant-a".to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: serde_json::to_string(&action).unwrap(),
                file_change_result_json: Some(serde_json::to_string(&terminal_result).unwrap()),
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&terminal_tool_result).unwrap()),
                error: None,
                created_at: 20,
                decided_at: Some(20),
                completed_at: Some(21),
                effective_permissions_json: Some("{}".to_string()),
                path_scope: Some(canonical_target.to_string_lossy().into_owned()),
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            },
        )
        .unwrap()
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-completed-direct-apply-patch",
        &source.id,
        "assistant-a",
        100,
    )
    .unwrap();
    assert!(plan.file_changes.is_empty());
    assert_eq!(plan.action_audits.len(), 1);
    let target_transaction_id = plan.id_replacements[SOURCE_TRANSACTION_ID].clone();
    let target_observation_id = plan.id_replacements[&observation_id].clone();
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_run_id = plan.run_id_map["run-source-0"].clone();
    let target_call_id = plan.id_replacements[APPLY_CALL_ID].clone();
    let target_audit = agent_action_audit_repository::load_action_audit_record(
        &connection,
        &crate::canonical_pending_action_id(&target_run_id, &target_call_id),
    )
    .unwrap()
    .unwrap();
    let target_action: AgentProposedAction =
        serde_json::from_str(&target_audit.action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: target_change,
    } = target_action
    else {
        unreachable!()
    };
    assert_eq!(target_change.id, target_call_id);
    assert_eq!(target_change.transaction_id, target_transaction_id);
    assert_eq!(target_change.inline_diff.unwrap().patch, inline_patch);
    assert_eq!(target_change.execution.conversation_id, plan.target.id);
    assert_eq!(target_change.execution.run_id, target_run_id);
    assert_eq!(
        target_change.execution.observation_id,
        target_observation_id
    );
    assert_eq!(
        target_change.execution.source_args_digest,
        source_args_digest
    );
    let target_result: crate::AgentFileChangeResult =
        serde_json::from_str(target_audit.file_change_result_json.as_deref().unwrap()).unwrap();
    assert_eq!(target_result.transaction_id, target_transaction_id);
    let target_tool_result: AgentToolResult =
        serde_json::from_str(target_audit.tool_result_json.as_deref().unwrap()).unwrap();
    assert_eq!(target_tool_result.call_id, target_call_id);
    assert_eq!(
        target_tool_result.result.as_ref().unwrap()["transactionId"],
        target_transaction_id
    );
    let target_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &plan.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    let target_result_transaction_id = target_trace.items.iter().find_map(|item| match item {
        ConversationTurnTraceItem::ToolResult {
            tool, observation, ..
        } if tool == "apply_patch" => observation["transactionId"].as_str(),
        _ => None,
    });
    assert_eq!(
        target_result_transaction_id,
        Some(target_transaction_id.as_str())
    );
}
