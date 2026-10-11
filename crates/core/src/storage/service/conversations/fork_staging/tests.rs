use super::*;
use crate::storage::models::AttachmentRecord;

fn fixture() -> (tempfile::TempDir, Arc<StorageService>) {
    let dir = tempfile::tempdir().unwrap();
    let service = StorageService::open_with_model_credentials(
        &dir.path().join("fork.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    service
        .save_conversation(ChatConversationRecord {
            id: "source".into(),
            project_id: None,
            model_id: None,
            title: "source".into(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "assistant".into(),
                role: "assistant".into(),
                content: "complete".into(),
                created_at: 1,
                status: Some("sent".into()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let relative = "conversations/source/assistant/source-attachment/example.txt";
    let path = service.attachment_root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"fork contents").unwrap();
    let connection = service.state.connection().unwrap();
    attachment_repository::save_attachment(
        &connection,
        &AttachmentRecord {
            id: "source-attachment".into(),
            conversation_id: "source".into(),
            message_id: "assistant".into(),
            project_id: None,
            kind: "file".into(),
            original_name: "example.txt".into(),
            mime_type: Some("text/plain".into()),
            size_bytes: 13,
            storage_rel_path: relative.into(),
            created_at: 1,
            pasted_text: None,
        },
    )
    .unwrap();
    drop(connection);
    (dir, Arc::new(service))
}

fn plan(service: &StorageService, request: &str) -> (ConversationForkPlan, ForkSourceVersions) {
    {
        let mut connection = service.state.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        conversation_fork_repository::recover_fork_source_continuations(
            &transaction,
            "source",
            &ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant".into(),
            },
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    let mut connection = service.state.read_connection().unwrap();
    let transaction = connection.transaction().unwrap();
    let plan = conversation_fork_repository::build_fork_plan_at_point_readonly(
        &transaction,
        request,
        "source",
        &ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant".into(),
        },
        2,
    )
    .unwrap();
    let versions = ForkSourceVersions::capture(service, &transaction, &plan).unwrap();
    transaction.commit().unwrap();
    (plan, versions)
}

fn commit(
    service: &StorageService,
    plan: &ConversationForkPlan,
    versions: &ForkSourceVersions,
) -> Result<String, ConversationForkError> {
    let mut connection = service.state.connection().unwrap();
    conversation_fork_repository::commit_prepared_fork_plan(
        &mut connection,
        plan,
        &[],
        |transaction| versions.validate(service, transaction, plan),
    )
}

#[test]
fn fork_copy_does_not_hold_storage_connection() {
    let (_dir, service) = fixture();
    let writer = service.clone();
    PREPARE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let (done, received) = std::sync::mpsc::channel();
            let task = std::thread::spawn(move || {
                let result = writer.state.connection().unwrap().execute(
                    "UPDATE conversations SET title='independent metadata' WHERE id='source'",
                    [],
                );
                done.send(result).unwrap();
            });
            received
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("fork staging blocked an independent writer")
                .unwrap();
            task.join().unwrap();
        }))
    });
    let result = service.fork_conversation_request_view(ForkConversationRequest {
        request_id: "parallel".into(),
        source_conversation_id: "source".into(),
        fork_point: ConversationForkPoint::AssistantReply {
            assistant_message_id: "assistant".into(),
        },
    });
    assert!(result.unwrap_err().message().contains("准备期间已变化"));
}

#[test]
fn fork_history_revision_and_source_deletion_are_rechecked_after_staging() {
    for delete in [false, true] {
        let (_dir, service) = fixture();
        let (mut plan, versions) = plan(&service, "changed");
        let files = ForkFiles::prepare(&service.attachment_root, &mut plan).unwrap();
        let own_path = files.published[0].clone();
        let connection = service.state.connection().unwrap();
        if delete {
            connection
                .execute("DELETE FROM conversations WHERE id='source'", [])
                .unwrap();
        } else {
            connection
                .execute(
                    "UPDATE messages SET content='changed' WHERE id='assistant'",
                    [],
                )
                .unwrap();
        }
        drop(connection);
        assert!(commit(&service, &plan, &versions)
            .unwrap_err()
            .message()
            .contains("准备期间已变化"));
        assert!(service
            .load_conversation(&plan.target.id)
            .unwrap()
            .is_none());
        drop(files);
        assert!(!own_path.exists());
    }
}

#[test]
fn concurrent_fork_receipt_reuses_winner_without_removing_its_files() {
    let (_dir, service) = fixture();
    let (mut first, first_versions) = plan(&service, "same-request");
    let (mut second, second_versions) = plan(&service, "same-request");
    let mut first_files = ForkFiles::prepare(&service.attachment_root, &mut first).unwrap();
    let second_files = ForkFiles::prepare(&service.attachment_root, &mut second).unwrap();
    let winner_path = first_files.published[0].clone();
    let loser_path = second_files.published[0].clone();
    assert_eq!(
        commit(&service, &first, &first_versions).unwrap(),
        first.target.id
    );
    first_files.mark_committed();
    // A committed idempotent receipt is not invalidated by a later source edit.
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET content='new history' WHERE id='assistant'",
            [],
        )
        .unwrap();
    assert_eq!(
        commit(&service, &second, &second_versions).unwrap(),
        first.target.id
    );
    drop(first_files);
    drop(second_files);
    assert_eq!(fs::read(winner_path).unwrap(), b"fork contents");
    assert!(!loser_path.exists());
    assert!(service
        .load_conversation(&second.target.id)
        .unwrap()
        .is_none());
}

#[test]
fn failed_fork_commit_cleans_its_files_and_keeps_source_and_other_operations() {
    let (_dir, service) = fixture();
    let (mut plan, versions) = plan(&service, "fail-commit");
    let files = ForkFiles::prepare(&service.attachment_root, &mut plan).unwrap();
    let own_path = files.published[0].clone();
    let unrelated = own_path.parent().unwrap().join("another-operation.forking");
    fs::write(&unrelated, "untouched").unwrap();
    service.state.connection().unwrap().execute_batch(
        "CREATE TRIGGER fail_fork_insert BEFORE INSERT ON conversations WHEN NEW.id <> 'source' BEGIN SELECT RAISE(ABORT, 'injected fork failure'); END;",
    ).unwrap();
    assert!(commit(&service, &plan, &versions)
        .unwrap_err()
        .message()
        .contains("injected fork failure"));
    drop(files);
    assert!(!own_path.exists());
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "untouched");
    assert!(service.load_conversation("source").unwrap().is_some());
    assert!(service
        .load_conversation(&plan.target.id)
        .unwrap()
        .is_none());
}

#[test]
fn attachment_publication_never_replaces_an_existing_target() {
    let (_dir, service) = fixture();
    let (mut plan, _) = plan(&service, "collision");
    let attachment = plan.attachments_mut().next().unwrap();
    let path = service.attachment_root.join(attachment_storage_rel_path(
        &attachment.target.conversation_id,
        &attachment.target.message_id,
        &attachment.target.id,
        &attachment.target.original_name,
    ));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "already owned").unwrap();
    assert!(ForkFiles::prepare(&service.attachment_root, &mut plan).is_err());
    assert_eq!(fs::read_to_string(path).unwrap(), "already owned");
}

#[test]
fn committed_fork_attachment_survives_storage_reopen() {
    let (dir, service) = fixture();
    let fork = service
        .fork_conversation_request_view(ForkConversationRequest {
            request_id: "restart".into(),
            source_conversation_id: "source".into(),
            fork_point: ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant".into(),
            },
        })
        .unwrap();
    let attachment = attachment_repository::list_conversation_attachments(
        &service.state.connection().unwrap(),
        &fork.conversation.id,
    )
    .unwrap()
    .pop()
    .unwrap();
    let path = service.attachment_root.join(attachment.storage_rel_path);
    drop(service);
    let reopened = StorageService::open_with_model_credentials(
        &dir.path().join("fork.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    assert!(reopened
        .load_conversation(&fork.conversation.id)
        .unwrap()
        .is_some());
    assert_eq!(fs::read(path).unwrap(), b"fork contents");
}

#[test]
fn fork_side_fence_rejects_diff_changes_without_a_timestamp_change() {
    let (_dir, service) = fixture();
    service
        .save_project(ProjectRecord::without_folders("project", "Project", 1))
        .unwrap();
    let identity = crate::AgentTurnDiffIdentity {
        run_id: "same-clock-run".into(),
        conversation_id: "source".into(),
        assistant_message_id: "assistant".into(),
        project_id: "project".into(),
        workspace_root: service.attachment_root.to_string_lossy().into_owned(),
    };
    service
        .replace_conversation_turn_trace(
            &crate::ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: identity.run_id.clone(),
                conversation_id: identity.conversation_id.clone(),
                assistant_message_id: identity.assistant_message_id.clone(),
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            1,
            2,
        )
        .unwrap();
    let change = crate::AgentTurnFileChange {
        path: "same-clock.txt".into(),
        before: crate::AgentTurnFileContent::Missing,
        after: crate::AgentTurnFileContent::Text("first version".into()),
    };
    {
        let mut connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE conversations SET project_id='project' WHERE id='source'",
                [],
            )
            .unwrap();
        crate::storage::turn_diff_repository::record_file_change(
            &mut connection,
            &identity,
            "first",
            &change,
            100,
        )
        .unwrap();
    }
    let (plan, versions) = plan(&service, "same-clock-fork");
    {
        let mut connection = service.state.connection().unwrap();
        crate::storage::turn_diff_repository::record_file_change(
            &mut connection,
            &identity,
            "second",
            &crate::AgentTurnFileChange {
                after: crate::AgentTurnFileContent::Text("changed in same millisecond".into()),
                ..change
            },
            100,
        )
        .unwrap();
        assert_eq!(connection.query_row("SELECT updated_at FROM agent_turn_diffs WHERE assistant_message_id='assistant'",
            [], |row| row.get::<_, i64>(0)).unwrap(), 100);
    }
    assert!(commit(&service, &plan, &versions)
        .unwrap_err()
        .message()
        .contains("准备期间已变化"));
    assert!(service
        .load_conversation(&plan.target.id)
        .unwrap()
        .is_none());
}
