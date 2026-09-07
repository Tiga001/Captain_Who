use super::*;

#[test]
fn forked_root_uses_the_reserved_name_independent_of_complex_titles_and_source_identity() {
    for title in [
        "让子智能体发送“已完成”给父亲".to_string(),
        "长中文会话标题".repeat(80),
        "读取 [页面](http://127.0.0.1:18765/path)\n标题".to_string(),
    ] {
        let fixture = Fixture::new(Some("model-a"));
        save_settled_history(&fixture, 1, false);
        let mut conversation = fixture
            .service
            .load_conversation("root-conversation")
            .unwrap()
            .unwrap();
        conversation.title = title.clone();
        fixture.service.save_conversation(conversation).unwrap();
        let source_root = fixture
            .service
            .get_agent_node_by_conversation("root-conversation")
            .unwrap()
            .unwrap();
        let fork = fixture
            .service
            .fork_conversation_request_view(ForkConversationRequest {
                request_id: "fixed-root-name-fork".to_string(),
                source_conversation_id: "root-conversation".to_string(),
                fork_point: ConversationForkPoint::AssistantReply {
                    assistant_message_id: "root-assistant-0".to_string(),
                },
            })
            .unwrap();
        let fork_root = fixture
            .service
            .get_agent_node_by_conversation(&fork.conversation.id)
            .unwrap()
            .unwrap();
        assert_eq!(fork_root.task_name, crate::ROOT_AGENT_TASK_NAME);
        assert_eq!(
            fixture
                .service
                .load_conversation("root-conversation")
                .unwrap()
                .unwrap()
                .title,
            title
        );
        assert_eq!(
            fixture
                .service
                .get_agent_node_by_conversation("root-conversation")
                .unwrap()
                .unwrap(),
            source_root
        );
        assert_ne!(fork_root.agent_id, source_root.agent_id);
    }
}

#[test]
fn three_level_agent_tree_fork_is_recursive_idempotent_and_independent() {
    const FIRST_FORK_REQUEST: &str = "three-level-tree-fork";
    const SECOND_FORK_REQUEST: &str = "three-level-tree-fork-recursive";
    const ROOT_BOUNDARY_CONTENT: &str = "three-level root fork boundary";
    const CHILD_FAILED_CONTENT: &str = "child failed before tree fork";
    const CHILD_AFTER_CUTOFF_CONTENT: &str = "child completed after tree fork cutoff";
    const GRANDCHILD_COMPLETED_CONTENT: &str = "grandchild completed before tree fork";
    const CHILD_TERMINAL_ERROR: &str = "child terminal failure before tree fork";

    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let child = fixture
        .service
        .create_child_agent(&spawn_input("tree-fork-child", "tree_fork_child"))
        .unwrap();
    let mut grandchild_input = spawn_input("tree-fork-grandchild", "tree_fork_grandchild");
    grandchild_input.parent_agent_id = child.agent.agent_id.clone();
    let grandchild = fixture
        .service
        .create_child_agent(&grandchild_input)
        .unwrap();
    for wake_id in [
        &child.initial_wake.wake_id,
        &grandchild.initial_wake.wake_id,
    ] {
        fixture
            .service
            .transition_agent_wake(
                wake_id,
                AgentWakeStatus::Queued,
                AgentWakeStatus::Cancelled,
                None,
            )
            .unwrap();
    }

    let member_history_at = now_ms().saturating_add(1);
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &child.agent.conversation_id,
        "tree-child-failed-before",
        "tree-child-run-before",
        CHILD_FAILED_CONTENT,
        ConversationTurnTraceTerminalStatus::Failed,
        Some(CHILD_TERMINAL_ERROR),
        member_history_at,
    );
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &grandchild.agent.conversation_id,
        "tree-grandchild-completed-before",
        "tree-grandchild-run-before",
        GRANDCHILD_COMPLETED_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        member_history_at.saturating_add(1),
    );
    let fork_boundary_at = now_ms().max(member_history_at.saturating_add(2));
    append_terminal_assistant_for_tree_fork(
        &fixture,
        "root-conversation",
        "tree-root-boundary",
        "tree-root-boundary-run",
        ROOT_BOUNDARY_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        fork_boundary_at,
    );
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &child.agent.conversation_id,
        "tree-child-completed-after",
        "tree-child-run-after",
        CHILD_AFTER_CUTOFF_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        fork_boundary_at.saturating_add(1),
    );

    let source_tree = tree_by_task_path(&fixture, "agent-root");
    assert_eq!(source_tree.len(), 3);
    let request = ForkConversationRequest {
        request_id: FIRST_FORK_REQUEST.to_string(),
        source_conversation_id: "root-conversation".to_string(),
        fork_point: ConversationForkPoint::AssistantReply {
            assistant_message_id: "tree-root-boundary".to_string(),
        },
    };
    let first_fork = fixture
        .service
        .fork_conversation_request_view(request.clone())
        .unwrap();
    let first_root = fixture
        .service
        .get_agent_node_by_conversation(&first_fork.conversation.id)
        .unwrap()
        .expect("the first fork owns an independent root Agent");
    assert_eq!(first_root.task_name, crate::ROOT_AGENT_TASK_NAME);
    let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
    assert_eq!(first_tree.len(), 3);
    assert_tree_identities_are_fresh(&fixture, &source_tree, &first_tree);

    let first_child = first_tree
        .get(&child.agent.task_path)
        .expect("the child task path is preserved");
    let first_grandchild = first_tree
        .get(&grandchild.agent.task_path)
        .expect("the grandchild task path is preserved");
    assert_eq!(
        first_child.parent_agent_id.as_deref(),
        Some(first_root.agent_id.as_str())
    );
    assert_eq!(
        first_grandchild.parent_agent_id.as_deref(),
        Some(first_child.agent_id.as_str())
    );
    assert!(first_tree.values().all(|member| {
        member.root_agent_id == first_root.agent_id
            && member.root_conversation_id == first_root.conversation_id
    }));

    let first_child_conversation = fixture
        .service
        .load_conversation(&first_child.conversation_id)
        .unwrap()
        .unwrap();
    let first_child_failed = first_child_conversation
        .messages
        .iter()
        .find(|message| message.content == CHILD_FAILED_CONTENT)
        .expect("the pre-cutoff failed child reply is cloned");
    assert_eq!(first_child_failed.status.as_deref(), Some("failed"));
    assert!(first_child_conversation
        .messages
        .iter()
        .all(|message| message.content != CHILD_AFTER_CUTOFF_CONTENT));
    let first_child_trace = fixture
        .service
        .get_conversation_turn_trace(&first_child_failed.id)
        .unwrap()
        .expect("the child terminal trace is cloned");
    assert_eq!(
        first_child_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(
        first_child_trace.terminal_error.as_deref(),
        Some(CHILD_TERMINAL_ERROR)
    );
    assert_eq!(first_child_trace.items.len(), 1);
    assert_ne!(first_child_trace.run_id, "tree-child-run-before");

    let first_grandchild_conversation = fixture
        .service
        .load_conversation(&first_grandchild.conversation_id)
        .unwrap()
        .unwrap();
    let first_grandchild_completed = first_grandchild_conversation
        .messages
        .iter()
        .find(|message| message.content == GRANDCHILD_COMPLETED_CONTENT)
        .expect("the pre-cutoff grandchild reply is cloned");
    let first_grandchild_trace = fixture
        .service
        .get_conversation_turn_trace(&first_grandchild_completed.id)
        .unwrap()
        .expect("the grandchild terminal trace is cloned");
    assert_eq!(
        first_grandchild_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert_ne!(first_grandchild_trace.run_id, "tree-grandchild-run-before");
    assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 2);

    let retried = fixture
        .service
        .fork_conversation_request_view(request)
        .unwrap();
    assert_eq!(retried.conversation.id, first_fork.conversation.id);
    assert_eq!(
        tree_by_task_path(&fixture, &first_root.root_agent_id),
        first_tree
    );
    assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 2);

    let first_root_conversation = fixture
        .service
        .load_conversation(&first_root.conversation_id)
        .unwrap()
        .unwrap();
    let first_root_boundary = first_root_conversation
        .messages
        .iter()
        .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
        .expect("the first fork preserves its root boundary");
    let second_fork = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: SECOND_FORK_REQUEST.to_string(),
            source_conversation_id: first_root.conversation_id.clone(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: first_root_boundary.id.clone(),
            },
        })
        .unwrap();
    let second_root = fixture
        .service
        .get_agent_node_by_conversation(&second_fork.conversation.id)
        .unwrap()
        .expect("the recursive fork owns another independent root Agent");
    let second_tree = tree_by_task_path(&fixture, &second_root.root_agent_id);
    assert_eq!(second_tree.len(), 3);
    assert_tree_identities_are_fresh(&fixture, &first_tree, &second_tree);
    assert_tree_identities_are_fresh(&fixture, &source_tree, &second_tree);
    let second_grandchild = second_tree
        .get(&grandchild.agent.task_path)
        .expect("the recursive fork preserves the grandchild task path");
    let second_grandchild_conversation = fixture
        .service
        .load_conversation(&second_grandchild.conversation_id)
        .unwrap()
        .unwrap();
    let second_grandchild_completed = second_grandchild_conversation
        .messages
        .iter()
        .find(|message| message.content == GRANDCHILD_COMPLETED_CONTENT)
        .expect("the recursive fork preserves grandchild history");
    let second_grandchild_trace = fixture
        .service
        .get_conversation_turn_trace(&second_grandchild_completed.id)
        .unwrap()
        .expect("the recursive fork preserves the grandchild trace");
    assert_eq!(
        second_grandchild_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert_ne!(
        second_grandchild_trace.run_id,
        first_grandchild_trace.run_id
    );
    assert_eq!(member_fork_receipt_count(&fixture, SECOND_FORK_REQUEST), 2);

    fixture
        .service
        .delete_conversation("root-conversation")
        .unwrap();
    for source in source_tree.values() {
        assert!(fixture
            .service
            .load_conversation(&source.conversation_id)
            .unwrap()
            .is_none());
        assert!(fixture
            .service
            .get_agent_node(&source.agent_id)
            .unwrap()
            .is_none());
    }
    for independent in first_tree.values().chain(second_tree.values()) {
        assert!(fixture
            .service
            .load_conversation(&independent.conversation_id)
            .unwrap()
            .is_some());
        assert_eq!(
            fixture
                .service
                .get_agent_node(&independent.agent_id)
                .unwrap()
                .as_ref(),
            Some(independent)
        );
    }
    let connection = fixture.service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn member_agent_fork_preserves_attachment_and_complete_turn_diff_recursively() {
    const MEMBER_CONTENT: &str = "member reply with attachment and turn diff";
    const ROOT_BOUNDARY_CONTENT: &str = "root boundary after member artifacts";
    const SOURCE_MESSAGE_ID: &str = "member-artifacts-assistant";
    const SOURCE_RUN_ID: &str = "member-artifacts-run";
    const SOURCE_ATTACHMENT_ID: &str = "member-artifacts-attachment";
    const SOURCE_BYTES: &[u8] = b"durable member attachment bytes";
    const ACTION_IDS: [&str; 2] = ["member-action-binary", "member-action-text"];

    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let child = fixture
        .service
        .create_child_agent(&spawn_input(
            "member-artifacts-child",
            "member_artifacts_child",
        ))
        .unwrap();
    fixture
        .service
        .transition_agent_wake(
            &child.initial_wake.wake_id,
            AgentWakeStatus::Queued,
            AgentWakeStatus::Cancelled,
            None,
        )
        .unwrap();

    let member_history_at = now_ms().saturating_add(1);
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &child.agent.conversation_id,
        SOURCE_MESSAGE_ID,
        SOURCE_RUN_ID,
        MEMBER_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        member_history_at,
    );
    let source_attachment = attach_file_to_tree_member_message(
        &fixture,
        &child.agent.conversation_id,
        SOURCE_MESSAGE_ID,
        SOURCE_ATTACHMENT_ID,
        SOURCE_BYTES,
        member_history_at,
    );
    let workspace_root = fixture
        ._directory
        .path()
        .join("project-a")
        .to_string_lossy()
        .to_string();
    let source_diff_identity = crate::AgentTurnDiffIdentity {
        run_id: SOURCE_RUN_ID.to_string(),
        conversation_id: child.agent.conversation_id.clone(),
        assistant_message_id: SOURCE_MESSAGE_ID.to_string(),
        project_id: "project-a".to_string(),
        workspace_root: workspace_root.clone(),
    };
    fixture
        .service
        .initialize_agent_turn_diff(&source_diff_identity)
        .unwrap();
    let expected_files = vec![
        crate::AgentTurnFileChange {
            path: "assets/member.bin".to_string(),
            before: crate::AgentTurnFileContent::Missing,
            after: crate::AgentTurnFileContent::Binary,
        },
        crate::AgentTurnFileChange {
            path: "src/member.rs".to_string(),
            before: crate::AgentTurnFileContent::Text("before member edit\n".to_string()),
            after: crate::AgentTurnFileContent::Text("after member edit\n".to_string()),
        },
    ];
    for (action_id, change) in ACTION_IDS.iter().zip(&expected_files) {
        assert!(fixture
            .service
            .record_agent_turn_file_change(&source_diff_identity, action_id, change)
            .unwrap());
    }
    {
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            connection
                .execute(
                    "UPDATE agent_turn_diffs
                         SET truncated = 1, updated_at = MAX(updated_at, ?2)
                         WHERE assistant_message_id = ?1",
                    rusqlite::params![SOURCE_MESSAGE_ID, member_history_at.saturating_add(1)],
                )
                .unwrap(),
            1
        );
    }
    assert_eq!(
        turn_diff_action_ids(&fixture, SOURCE_MESSAGE_ID),
        ACTION_IDS.map(ToString::to_string)
    );

    let fork_boundary_at = now_ms().max(member_history_at.saturating_add(2));
    append_terminal_assistant_for_tree_fork(
        &fixture,
        "root-conversation",
        "member-artifacts-root-boundary",
        "member-artifacts-root-run",
        ROOT_BOUNDARY_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        fork_boundary_at,
    );
    let first_fork = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "member-artifacts-first-fork".to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "member-artifacts-root-boundary".to_string(),
            },
        })
        .unwrap();
    let first_root = fixture
        .service
        .get_agent_node_by_conversation(&first_fork.conversation.id)
        .unwrap()
        .unwrap();
    let source_tree = tree_by_task_path(&fixture, "agent-root");
    let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
    assert_tree_identities_are_fresh(&fixture, &source_tree, &first_tree);
    let first_child = first_tree.get(&child.agent.task_path).unwrap();
    let first_child_conversation = fixture
        .service
        .load_conversation(&first_child.conversation_id)
        .unwrap()
        .unwrap();
    let first_message = first_child_conversation
        .messages
        .iter()
        .find(|message| message.content == MEMBER_CONTENT)
        .expect("the member artifact owner message is cloned");
    assert_ne!(first_message.id, SOURCE_MESSAGE_ID);
    assert_eq!(first_message.attachments.len(), 1);
    assert_ne!(first_message.attachments[0].id, SOURCE_ATTACHMENT_ID);
    let first_attachment = {
        let connection = fixture.service.state.connection().unwrap();
        attachment_repository::list_conversation_attachments(
            &connection,
            &first_child.conversation_id,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
    };
    assert_eq!(first_message.attachments[0].id, first_attachment.id);
    assert_eq!(first_attachment.message_id, first_message.id);
    assert_eq!(
        first_attachment.conversation_id,
        first_child.conversation_id
    );
    assert_ne!(first_attachment.id, source_attachment.id);
    assert_ne!(
        first_attachment.storage_rel_path,
        source_attachment.storage_rel_path
    );
    assert_eq!(
        first_attachment.storage_rel_path,
        slash_path(&attachment_storage_rel_path(
            &first_child.conversation_id,
            &first_message.id,
            &first_attachment.id,
            &first_attachment.original_name,
        ))
    );
    assert_eq!(
        fs::read(
            fixture
                .service
                .attachment_root
                .join(&first_attachment.storage_rel_path)
        )
        .unwrap(),
        SOURCE_BYTES
    );

    let first_diffs = fixture
        .service
        .load_agent_turn_diffs_for_messages(
            &first_child.conversation_id,
            "project-a",
            std::slice::from_ref(&first_message.id),
        )
        .unwrap();
    assert_eq!(first_diffs.len(), 1);
    let first_diff = &first_diffs[0];
    assert_eq!(
        first_diff.identity.conversation_id,
        first_child.conversation_id
    );
    assert_eq!(first_diff.identity.assistant_message_id, first_message.id);
    assert_ne!(first_diff.identity.run_id, SOURCE_RUN_ID);
    assert_eq!(first_diff.identity.workspace_root, workspace_root);
    assert_eq!(first_diff.files, expected_files);
    assert!(first_diff.truncated);
    assert_eq!(
        turn_diff_action_ids(&fixture, &first_message.id),
        ACTION_IDS.map(ToString::to_string)
    );
    assert!(!fixture
        .service
        .record_agent_turn_file_change(&first_diff.identity, ACTION_IDS[0], &expected_files[0],)
        .unwrap());
    assert_eq!(
        member_fork_receipt_count(&fixture, "member-artifacts-first-fork"),
        1
    );

    let first_root_conversation = fixture
        .service
        .load_conversation(&first_root.conversation_id)
        .unwrap()
        .unwrap();
    let first_boundary = first_root_conversation
        .messages
        .iter()
        .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
        .unwrap();
    let recursive_fork = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "member-artifacts-recursive-fork".to_string(),
            source_conversation_id: first_root.conversation_id.clone(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: first_boundary.id.clone(),
            },
        })
        .unwrap();
    let recursive_root = fixture
        .service
        .get_agent_node_by_conversation(&recursive_fork.conversation.id)
        .unwrap()
        .unwrap();
    let recursive_tree = tree_by_task_path(&fixture, &recursive_root.root_agent_id);
    assert_tree_identities_are_fresh(&fixture, &first_tree, &recursive_tree);
    let recursive_child = recursive_tree.get(&child.agent.task_path).unwrap();
    let recursive_child_conversation = fixture
        .service
        .load_conversation(&recursive_child.conversation_id)
        .unwrap()
        .unwrap();
    let recursive_message = recursive_child_conversation
        .messages
        .iter()
        .find(|message| message.content == MEMBER_CONTENT)
        .unwrap();
    let recursive_attachment = {
        let connection = fixture.service.state.connection().unwrap();
        attachment_repository::list_conversation_attachments(
            &connection,
            &recursive_child.conversation_id,
        )
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
    };
    assert_ne!(recursive_attachment.id, source_attachment.id);
    assert_ne!(recursive_attachment.id, first_attachment.id);
    assert_eq!(recursive_attachment.message_id, recursive_message.id);
    assert_eq!(recursive_message.attachments[0].id, recursive_attachment.id);
    assert_eq!(
        recursive_attachment.storage_rel_path,
        slash_path(&attachment_storage_rel_path(
            &recursive_child.conversation_id,
            &recursive_message.id,
            &recursive_attachment.id,
            &recursive_attachment.original_name,
        ))
    );
    assert_eq!(
        fs::read(
            fixture
                .service
                .attachment_root
                .join(&recursive_attachment.storage_rel_path)
        )
        .unwrap(),
        SOURCE_BYTES
    );
    let recursive_diff = fixture
        .service
        .load_agent_turn_diffs_for_messages(
            &recursive_child.conversation_id,
            "project-a",
            std::slice::from_ref(&recursive_message.id),
        )
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(recursive_diff.identity.run_id, first_diff.identity.run_id);
    assert_eq!(recursive_diff.files, expected_files);
    assert!(recursive_diff.truncated);
    assert_eq!(
        turn_diff_action_ids(&fixture, &recursive_message.id),
        ACTION_IDS.map(ToString::to_string)
    );
    assert_eq!(
        member_fork_receipt_count(&fixture, "member-artifacts-recursive-fork"),
        1
    );
}

#[test]
fn member_compaction_respects_root_cutoff_and_remains_recursive() {
    const VISIBLE_CONTENT: &str = "member reply before compaction cutoff";
    const AFTER_CUTOFF_CONTENT: &str = "member reply after compaction cutoff";
    const ROOT_BOUNDARY_CONTENT: &str = "root boundary between member compactions";
    const VISIBLE_MESSAGE_ID: &str = "member-compaction-visible-assistant";
    const VISIBLE_RUN_ID: &str = "member-compaction-visible-run";
    const VISIBLE_SUMMARY_ID: &str = "member-compaction-visible-summary";
    const VISIBLE_OPERATION_ID: &str = "member-compaction-visible-operation";
    const AFTER_MESSAGE_ID: &str = "member-compaction-after-assistant";
    const AFTER_RUN_ID: &str = "member-compaction-after-run";
    const AFTER_SUMMARY_ID: &str = "member-compaction-after-summary";
    const AFTER_OPERATION_ID: &str = "member-compaction-after-operation";
    const FIRST_FORK_REQUEST: &str = "member-compaction-first-fork";
    const RECURSIVE_FORK_REQUEST: &str = "member-compaction-recursive-fork";

    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let child_input = spawn_input("member-compaction-child", "member_compaction_child");
    let child = fixture.service.create_child_agent(&child_input).unwrap();
    fixture
        .service
        .transition_agent_wake(
            &child.initial_wake.wake_id,
            AgentWakeStatus::Queued,
            AgentWakeStatus::Cancelled,
            None,
        )
        .unwrap();
    let member_model_id = child
        .agent
        .model_snapshot
        .as_ref()
        .unwrap()
        .model_config_id
        .clone();

    let visible_at = now_ms().saturating_add(1);
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &child.agent.conversation_id,
        VISIBLE_MESSAGE_ID,
        VISIBLE_RUN_ID,
        VISIBLE_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        visible_at,
    );
    let visible_receipt = {
        let mut connection = fixture.service.state.connection().unwrap();
        let prefix = crate::storage::context_compaction_repository::prepare_prefix(
            &connection,
            &child.agent.conversation_id,
            &crate::ContextJournalCursor::message(&child.task_message.projection_message_id),
        )
        .unwrap();
        record_applied_member_compaction(
            &mut connection,
            &prefix,
            VISIBLE_SUMMARY_ID,
            VISIBLE_OPERATION_ID,
            VISIBLE_RUN_ID,
            VISIBLE_MESSAGE_ID,
            &member_model_id,
            visible_at.saturating_add(1),
        )
    };

    let root_cutoff_at = now_ms().max(visible_at.saturating_add(3));
    append_terminal_assistant_for_tree_fork(
        &fixture,
        "root-conversation",
        "member-compaction-root-boundary",
        "member-compaction-root-run",
        ROOT_BOUNDARY_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        root_cutoff_at,
    );
    append_terminal_assistant_for_tree_fork(
        &fixture,
        &child.agent.conversation_id,
        AFTER_MESSAGE_ID,
        AFTER_RUN_ID,
        AFTER_CUTOFF_CONTENT,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
        root_cutoff_at.saturating_add(1),
    );
    let after_cutoff_receipt = {
        let mut connection = fixture.service.state.connection().unwrap();
        let prefix = crate::storage::context_compaction_repository::prepare_prefix(
            &connection,
            &child.agent.conversation_id,
            &crate::ContextJournalCursor::message(VISIBLE_MESSAGE_ID),
        )
        .unwrap();
        record_applied_member_compaction(
            &mut connection,
            &prefix,
            AFTER_SUMMARY_ID,
            AFTER_OPERATION_ID,
            AFTER_RUN_ID,
            AFTER_MESSAGE_ID,
            &member_model_id,
            root_cutoff_at.saturating_add(2),
        )
    };
    {
        let connection = fixture.service.state.connection().unwrap();
        assert_eq!(
            crate::storage::context_compaction_repository::list_active_summary_chain(
                &connection,
                &child.agent.conversation_id,
            )
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                &connection,
                &child.agent.conversation_id,
            )
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
                crate::storage::model_request_observation_repository::list_observations_for_conversation(
                    &connection,
                    &child.agent.conversation_id,
                )
                .unwrap()
                .len(),
                2
            );
    }

    let first_fork = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: FIRST_FORK_REQUEST.to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "member-compaction-root-boundary".to_string(),
            },
        })
        .unwrap();
    let first_root = fixture
        .service
        .get_agent_node_by_conversation(&first_fork.conversation.id)
        .unwrap()
        .unwrap();
    let first_tree = tree_by_task_path(&fixture, &first_root.root_agent_id);
    let first_child = first_tree.get(&child.agent.task_path).unwrap();
    let first_conversation = fixture
        .service
        .load_conversation(&first_child.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(first_conversation.messages.len(), 2);
    let first_task = first_conversation
        .messages
        .iter()
        .find(|message| message.content == child_input.task)
        .unwrap();
    let first_visible = first_conversation
        .messages
        .iter()
        .find(|message| message.content == VISIBLE_CONTENT)
        .unwrap();
    assert!(first_conversation
        .messages
        .iter()
        .all(|message| message.content != AFTER_CUTOFF_CONTENT));
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(&first_child.conversation_id, &first_task.id)
            .unwrap(),
        ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: child.agent.conversation_id.clone(),
            source_message_id: child.task_message.projection_message_id.clone(),
            original: Box::new(ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: child.task_message.message_id.clone(),
            }),
        }
    );
    let first_visible_run_id =
        serde_json::from_str::<serde_json::Value>(first_visible.agent_run_json.as_deref().unwrap())
            .unwrap()["runId"]
            .as_str()
            .unwrap()
            .to_string();
    let (first_chain, first_receipts, first_observations, first_head) = {
        let connection = fixture.service.state.connection().unwrap();
        let snapshot_count = connection
            .query_row(
                "SELECT COUNT(*) FROM messages
                     WHERE conversation_id = ?1 AND input_origin_kind = 'snapshot'",
                [&first_child.conversation_id],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        assert_eq!(snapshot_count, first_conversation.messages.len());
        let chain = crate::storage::context_compaction_repository::list_active_summary_chain(
            &connection,
            &first_child.conversation_id,
        )
        .unwrap();
        let receipts =
            crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap();
        let observations = crate::storage::model_request_observation_repository::list_observations_for_conversation(
                &connection,
                &first_child.conversation_id,
            )
            .unwrap();
        let head = connection
            .query_row(
                "SELECT summary_id FROM conversation_context_compaction_heads
                     WHERE conversation_id = ?1",
                [&first_child.conversation_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        (chain, receipts, observations, head)
    };
    assert_eq!(first_chain.len(), 1);
    let first_summary = &first_chain[0];
    assert_ne!(first_summary.summary.id, VISIBLE_SUMMARY_ID);
    assert_ne!(first_summary.summary.id, AFTER_SUMMARY_ID);
    assert_eq!(
        first_summary.lineage.source_summary_id.as_deref(),
        visible_receipt.summary_id.as_deref()
    );
    assert_ne!(
        first_summary.lineage.source_summary_id.as_deref(),
        after_cutoff_receipt.summary_id.as_deref()
    );
    assert_eq!(
        first_summary.summary.content,
        format!("summary {VISIBLE_SUMMARY_ID}")
    );
    assert_eq!(
        first_summary.summary.conversation_id,
        first_child.conversation_id
    );
    assert_eq!(
        first_summary.summary.covered_through,
        crate::ContextJournalCursor::message(&first_task.id)
    );
    assert_eq!(first_head, first_summary.summary.id);

    assert_eq!(first_receipts.len(), 1);
    let first_receipt = &first_receipts[0];
    assert!(!first_receipt
        .operation_id
        .starts_with("provider-transition-"));
    assert_ne!(first_receipt.operation_id, visible_receipt.operation_id);
    assert_ne!(
        first_receipt.operation_id,
        after_cutoff_receipt.operation_id
    );
    assert_eq!(first_receipt.run_id, first_visible_run_id);
    assert_ne!(first_receipt.run_id, visible_receipt.run_id);
    assert_eq!(first_receipt.conversation_id, first_child.conversation_id);
    assert_eq!(first_receipt.assistant_message_id, first_visible.id);
    assert_eq!(
        first_receipt.status,
        crate::ContextCompactionReceiptStatus::Applied
    );
    assert_eq!(
        first_receipt.stage,
        crate::ContextCompactionReceiptStage::Completed
    );
    assert_eq!(
        first_receipt.plan.covered_through,
        crate::ContextJournalCursor::message(&first_task.id)
    );
    assert_eq!(
        first_receipt.summary_id.as_deref(),
        Some(first_summary.summary.id.as_str())
    );
    assert_eq!(first_receipt.completed_at, visible_receipt.completed_at);
    assert_eq!(first_observations.len(), 1);
    let first_observation = &first_observations[0];
    assert_eq!(
        first_receipt.generation_observation_id.as_deref(),
        Some(first_observation.id.as_str())
    );
    assert_ne!(
        first_observation.id,
        visible_receipt.generation_observation_id.clone().unwrap()
    );
    assert_eq!(first_observation.run_id, first_receipt.run_id);
    assert_eq!(
        first_observation.conversation_id.as_deref(),
        Some(first_child.conversation_id.as_str())
    );
    assert_eq!(
        first_observation.assistant_message_id.as_deref(),
        Some(first_visible.id.as_str())
    );
    assert_eq!(
        first_observation.operation_id.as_deref(),
        Some(first_receipt.operation_id.as_str())
    );
    assert_eq!(
        first_observation.purpose,
        crate::ModelRequestPurpose::ContextCompaction
    );
    assert_eq!(member_fork_receipt_count(&fixture, FIRST_FORK_REQUEST), 1);

    let first_root_conversation = fixture
        .service
        .load_conversation(&first_root.conversation_id)
        .unwrap()
        .unwrap();
    let first_root_boundary = first_root_conversation
        .messages
        .iter()
        .find(|message| message.content == ROOT_BOUNDARY_CONTENT)
        .unwrap();
    let recursive_fork = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: RECURSIVE_FORK_REQUEST.to_string(),
            source_conversation_id: first_root.conversation_id.clone(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: first_root_boundary.id.clone(),
            },
        })
        .unwrap();
    let recursive_root = fixture
        .service
        .get_agent_node_by_conversation(&recursive_fork.conversation.id)
        .unwrap()
        .unwrap();
    let recursive_tree = tree_by_task_path(&fixture, &recursive_root.root_agent_id);
    let recursive_child = recursive_tree.get(&child.agent.task_path).unwrap();
    let recursive_conversation = fixture
        .service
        .load_conversation(&recursive_child.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(recursive_conversation.messages.len(), 2);
    let recursive_task = recursive_conversation
        .messages
        .iter()
        .find(|message| message.content == child_input.task)
        .unwrap();
    let recursive_visible = recursive_conversation
        .messages
        .iter()
        .find(|message| message.content == VISIBLE_CONTENT)
        .unwrap();
    assert!(recursive_conversation
        .messages
        .iter()
        .all(|message| message.content != AFTER_CUTOFF_CONTENT));
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(&recursive_child.conversation_id, &recursive_task.id)
            .unwrap(),
        ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: first_child.conversation_id.clone(),
            source_message_id: first_task.id.clone(),
            original: Box::new(ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: child.task_message.message_id.clone(),
            }),
        }
    );
    let recursive_visible_run_id = serde_json::from_str::<serde_json::Value>(
        recursive_visible.agent_run_json.as_deref().unwrap(),
    )
    .unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_string();
    let (recursive_chain, recursive_receipts, recursive_observations, recursive_head) = {
        let connection = fixture.service.state.connection().unwrap();
        let snapshot_count = connection
            .query_row(
                "SELECT COUNT(*) FROM messages
                     WHERE conversation_id = ?1 AND input_origin_kind = 'snapshot'",
                [&recursive_child.conversation_id],
                |row| row.get::<_, usize>(0),
            )
            .unwrap();
        assert_eq!(snapshot_count, recursive_conversation.messages.len());
        let chain = crate::storage::context_compaction_repository::list_active_summary_chain(
            &connection,
            &recursive_child.conversation_id,
        )
        .unwrap();
        let receipts =
            crate::storage::context_compaction_receipt_repository::list_receipts_for_conversation(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap();
        let observations = crate::storage::model_request_observation_repository::list_observations_for_conversation(
                &connection,
                &recursive_child.conversation_id,
            )
            .unwrap();
        let head = connection
            .query_row(
                "SELECT summary_id FROM conversation_context_compaction_heads
                     WHERE conversation_id = ?1",
                [&recursive_child.conversation_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        (chain, receipts, observations, head)
    };
    assert_eq!(recursive_chain.len(), 1);
    let recursive_summary = &recursive_chain[0];
    assert_ne!(recursive_summary.summary.id, first_summary.summary.id);
    assert_eq!(
        recursive_summary.lineage.source_summary_id.as_deref(),
        Some(first_summary.summary.id.as_str())
    );
    assert_eq!(
        recursive_summary.summary.content,
        first_summary.summary.content
    );
    assert_eq!(
        recursive_summary.summary.covered_through,
        crate::ContextJournalCursor::message(&recursive_task.id)
    );
    assert_eq!(recursive_head, recursive_summary.summary.id);
    assert_eq!(recursive_receipts.len(), 1);
    let recursive_receipt = &recursive_receipts[0];
    assert_ne!(recursive_receipt.operation_id, first_receipt.operation_id);
    assert_eq!(recursive_receipt.run_id, recursive_visible_run_id);
    assert_ne!(recursive_receipt.run_id, first_receipt.run_id);
    assert_eq!(
        recursive_receipt.conversation_id,
        recursive_child.conversation_id
    );
    assert_eq!(recursive_receipt.assistant_message_id, recursive_visible.id);
    assert_eq!(
        recursive_receipt.plan.covered_through,
        crate::ContextJournalCursor::message(&recursive_task.id)
    );
    assert_eq!(
        recursive_receipt.summary_id.as_deref(),
        Some(recursive_summary.summary.id.as_str())
    );
    assert_eq!(recursive_observations.len(), 1);
    let recursive_observation = &recursive_observations[0];
    assert_eq!(
        recursive_receipt.generation_observation_id.as_deref(),
        Some(recursive_observation.id.as_str())
    );
    assert_ne!(recursive_observation.id, first_observation.id);
    assert_eq!(recursive_observation.run_id, recursive_receipt.run_id);
    assert_eq!(
        recursive_observation.conversation_id.as_deref(),
        Some(recursive_child.conversation_id.as_str())
    );
    assert_eq!(
        recursive_observation.assistant_message_id.as_deref(),
        Some(recursive_visible.id.as_str())
    );
    assert_eq!(
        recursive_observation.operation_id.as_deref(),
        Some(recursive_receipt.operation_id.as_str())
    );
    assert_eq!(
        member_fork_receipt_count(&fixture, RECURSIVE_FORK_REQUEST),
        1
    );
}

#[test]
fn active_root_fork_is_atomic_idempotent_and_preserves_agent_actor_without_crossing_trees() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let child = fixture
        .service
        .create_child_agent(&spawn_input("fork-child", "fork_child"))
        .unwrap();
    fixture
        .service
        .transition_agent_wake(
            &child.initial_wake.wake_id,
            AgentWakeStatus::Queued,
            AgentWakeStatus::Cancelled,
            None,
        )
        .unwrap();
    let result = EnqueueAgentMessageInput {
        message_id: "mailbox-fork-result".to_string(),
        root_agent_id: "agent-root".to_string(),
        sender_agent_id: child.agent.agent_id.clone(),
        recipient_agent_id: "agent-root".to_string(),
        request_id: "request-fork-result".to_string(),
        kind: AgentMailboxKind::Result,
        content: "internal child evidence".to_string(),
        projection_message_id: "projection-fork-result".to_string(),
    };
    fixture.service.enqueue_agent_message(&result).unwrap();
    fixture
        .service
        .claim_next_agent_message("agent-root", "claim-fork-result")
        .unwrap()
        .unwrap();
    fixture
        .service
        .acknowledge_agent_message_with_projection(&result.message_id, "claim-fork-result")
        .unwrap();
    let fork_boundary_at = now_ms().saturating_add(1);
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (
                         'root-assistant-after-result', 'root-conversation', 'assistant',
                         'combined answer', 'completed', ?1,
                         (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                          WHERE conversation_id = 'root-conversation')
                     )",
                [fork_boundary_at],
            )
            .unwrap();
    }
    let request = ForkConversationRequest {
        request_id: "collaboration-root-fork".to_string(),
        source_conversation_id: "root-conversation".to_string(),
        fork_point: ConversationForkPoint::AssistantReply {
            assistant_message_id: "root-assistant-after-result".to_string(),
        },
    };

    let created = fixture
        .service
        .fork_conversation_request_view(request.clone())
        .unwrap();
    assert!(created
        .conversation
        .messages
        .iter()
        .all(|message| message.content != "internal child evidence"));
    let target_id = created.conversation.id.clone();
    let target_root = fixture
        .service
        .get_agent_node_by_conversation(&target_id)
        .unwrap()
        .expect("fork target is bound to an independent root");
    assert_eq!(target_root.parent_agent_id, None);
    assert_eq!(target_root.root_conversation_id, target_id);
    assert_ne!(target_root.root_agent_id, "agent-root");
    assert_eq!(target_root.project_id.as_deref(), Some("project-a"));

    let raw_target = fixture
        .service
        .load_conversation(&target_id)
        .unwrap()
        .unwrap();
    let copied_agent_input = raw_target
        .messages
        .iter()
        .find(|message| message.content == "internal child evidence")
        .expect("transport fact remains available to context assembly");
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(&target_id, &copied_agent_input.id)
            .unwrap(),
        ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: "root-conversation".to_string(),
            source_message_id: result.projection_message_id.clone(),
            original: Box::new(ConversationMessageOrigin::Agent {
                sender_agent_id: child.agent.agent_id.clone(),
                source_agent_message_id: result.message_id.clone(),
            }),
        }
    );
    assert_eq!(
        fixture.service.list_agent_tree("agent-root").unwrap().len(),
        2
    );
    let target_tree = fixture
        .service
        .list_agent_tree(&target_root.root_agent_id)
        .unwrap();
    assert_eq!(target_tree.len(), 2);
    let target_child = target_tree
        .iter()
        .find(|agent| agent.parent_agent_id.is_some())
        .expect("visible source child is cloned into the independent target tree")
        .clone();
    assert_eq!(
        target_child.parent_agent_id.as_deref(),
        Some(target_root.agent_id.as_str())
    );
    assert_eq!(target_child.task_path, child.agent.task_path);
    assert_ne!(target_child.agent_id, child.agent.agent_id);
    assert_ne!(target_child.conversation_id, child.agent.conversation_id);
    // The immutable receipt remains the idempotency truth even if lifecycle display state
    // changes after the successful fork. Admission required an active source at creation;
    // retry must not attempt a second target or depend on mutable lifecycle.
    fixture
        .service
        .transition_agent_lifecycle(
            &target_child.agent_id,
            target_child.revision,
            AgentLifecycle::Active,
            AgentLifecycle::Disabled,
        )
        .unwrap();
    let target_root = fixture
        .service
        .transition_agent_lifecycle(
            &target_root.agent_id,
            target_root.revision,
            AgentLifecycle::Active,
            AgentLifecycle::Disabled,
        )
        .unwrap();

    let reopened = StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap();
    let retried = reopened.fork_conversation_request_view(request).unwrap();
    assert_eq!(retried.conversation.id, target_id);
    assert!(retried
        .conversation
        .messages
        .iter()
        .all(|message| message.content != "internal child evidence"));
    assert_eq!(
        reopened
            .get_agent_node_by_conversation(&target_id)
            .unwrap()
            .unwrap(),
        target_root
    );
    let receipt_update = reopened
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_forks
                 SET source_root_agent_id = target_root_agent_id
                 WHERE request_id = 'collaboration-root-fork'",
            [],
        )
        .unwrap_err();
    assert!(receipt_update
        .to_string()
        .contains("Conversation fork receipt is immutable"));
    let hidden_hits = reopened
        .search_chats(&ChatSearchInput {
            query: "internal child evidence".to_string(),
            limit: Some(10),
        })
        .unwrap();
    assert!(hidden_hits.is_empty());

    let child_fork = reopened
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "forbidden-child-fork".to_string(),
            source_conversation_id: child.agent.conversation_id,
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "missing".to_string(),
            },
        })
        .unwrap_err();
    assert!(child_fork.message().contains("子 Agent 保持只读"));
}

#[test]
fn deleting_a_root_conversation_removes_its_child_tree_but_preserves_an_independent_fork() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let child = fixture
        .service
        .create_child_agent(&spawn_input("delete-tree-child", "delete_tree_child"))
        .unwrap();
    let forked = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "delete-tree-independent-fork".to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "root-assistant-0".to_string(),
            },
        })
        .unwrap();
    let forked_root = fixture
        .service
        .get_agent_node_by_conversation(&forked.conversation.id)
        .unwrap()
        .unwrap();

    let child_delete_error = fixture
        .service
        .delete_conversation(&child.agent.conversation_id)
        .unwrap_err();
    assert!(child_delete_error.contains("owned by their root task"));

    fixture
        .service
        .delete_conversation("root-conversation")
        .unwrap();
    assert!(fixture
        .service
        .load_conversation("root-conversation")
        .unwrap()
        .is_none());
    assert!(fixture
        .service
        .load_conversation(&child.agent.conversation_id)
        .unwrap()
        .is_none());
    assert!(fixture
        .service
        .get_agent_node("agent-root")
        .unwrap()
        .is_none());
    assert!(fixture
        .service
        .get_agent_node(&child.agent.agent_id)
        .unwrap()
        .is_none());

    assert!(fixture
        .service
        .load_conversation(&forked.conversation.id)
        .unwrap()
        .is_some());
    assert_eq!(
        fixture
            .service
            .get_agent_node(&forked_root.agent_id)
            .unwrap(),
        Some(forked_root)
    );
    let connection = fixture.service.state.connection().unwrap();
    let violations = connection
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count();
    assert_eq!(violations, 0);
}

#[test]
fn active_root_turn_rejects_fork_without_writing_target_or_receipt() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, true);
    let before = {
        let connection = fixture.service.state.connection().unwrap();
        (
            connection
                .query_row("SELECT COUNT(*) FROM conversations", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT COUNT(*) FROM agent_nodes", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT COUNT(*) FROM conversation_forks", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
        )
    };
    let error = fixture
        .service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "active-root-fork".to_string(),
            source_conversation_id: "root-conversation".to_string(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "root-assistant-0".to_string(),
            },
        })
        .unwrap_err();
    assert!(error.message().contains("活跃 Turn"));
    let after = {
        let connection = fixture.service.state.connection().unwrap();
        (
            connection
                .query_row("SELECT COUNT(*) FROM conversations", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT COUNT(*) FROM agent_nodes", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            connection
                .query_row("SELECT COUNT(*) FROM conversation_forks", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
        )
    };
    assert_eq!(after, before);
}
