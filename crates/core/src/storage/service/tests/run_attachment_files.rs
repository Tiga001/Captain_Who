use super::*;

fn read_turn(service: &StorageService, id: &str, project: Option<&str>) {
    save_read_turn(service, conversation(id, project, &format!("{id}-user")));
}

fn save_read_turn(service: &StorageService, mut record: ChatConversationRecord) {
    let id = record.id.clone();
    let assistant_id = format!("{id}-assistant");
    record.messages.push(ChatMessageRecord {
        id: assistant_id.clone(),
        role: "assistant".into(),
        content: String::new(),
        created_at: 2,
        status: Some("completed".into()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: None,
        ui_state_json: None,
        human_interaction_response: None,
    });
    service.save_conversation(record).unwrap();
    service
        .replace_conversation_turn_trace(
            &crate::ConversationTurnTrace {
                schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: format!("{id}-run"),
                conversation_id: id.into(),
                assistant_message_id: assistant_id,
                terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            2,
            3,
        )
        .unwrap();
}

fn attach(
    service: &StorageService,
    conversation_id: &str,
    project: Option<&str>,
    id: &str,
) -> String {
    let attachment = input_attachment(
        service,
        id,
        AgentInputAttachmentKind::File,
        "evidence.txt",
        Some("text/plain"),
        b"inherited evidence",
    );
    service
        .save_input_attachments(
            conversation_id,
            &format!("{conversation_id}-user"),
            project,
            &[attachment],
            1,
        )
        .unwrap();
    attachment_read_path(id, "evidence.txt")
}

#[test]
fn run_attachment_file_resolves_inherited_tree_without_local_listing_receipts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    read_turn(&service, "parent", None);
    read_turn(&service, "unrelated", None);
    let mut child = conversation("child", None, "child-user");
    child.messages.clear();
    service.save_conversation(child.clone()).unwrap();
    bind_agent_root(&service, "root-agent", "parent");
    bind_agent_child(
        &service,
        "child-agent",
        "child",
        "root-agent",
        "parent",
        "child",
    );
    save_read_turn(&service, child);
    let path = attach(&service, "parent", None, "inherited-file");

    let (id, file) = service
        .resolve_run_attachment_file("child", "child-assistant", &path)
        .unwrap();
    assert_eq!(id, "inherited-file");
    assert_eq!(file.conversation_id, "parent");
    assert_eq!(file.message_id, "parent-user");
    assert_eq!(fs::read(&file.path).unwrap(), b"inherited evidence");
    for (conversation, assistant, source) in [
        ("child", "parent-assistant", path.as_str()),
        ("child", "missing-assistant", path.as_str()),
        ("unrelated", "unrelated-assistant", path.as_str()),
        (
            "child",
            "child-assistant",
            "@attachments/inherited-file/forged.txt",
        ),
        ("child", "child-assistant", "/outside/evidence.txt"),
    ] {
        assert!(service
            .resolve_run_attachment_file(conversation, assistant, source)
            .is_err());
    }
}

#[test]
fn run_attachment_file_preserves_project_scope_and_sent_owner_validation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    read_turn(&service, "owner", Some("project-1"));
    read_turn(&service, "reader", Some("project-1"));
    read_turn(&service, "foreign", Some("project-2"));
    let path = attach(&service, "owner", Some("project-1"), "project-file");
    let foreign_path = attach(&service, "foreign", Some("project-2"), "foreign-file");
    assert!(service
        .resolve_run_attachment_file("reader", "reader-assistant", &path)
        .is_ok());
    assert!(service
        .resolve_run_attachment_file("reader", "reader-assistant", &foreign_path)
        .is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET status = 'pending' WHERE id = 'owner-user'",
            [],
        )
        .unwrap();
    assert!(service
        .resolve_run_attachment_file("reader", "reader-assistant", &path)
        .is_err());
}
