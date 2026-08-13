use super::*;

fn test_input(permissions: AgentPermissions) -> AgentChatInput {
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://should-not-be-called.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-file-policy".to_string()),
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("file-policy-test".to_string()),
            root_path: Some("/tmp/file-policy-test".to_string()),
        }),
        attachment_library: None,
        permissions,
    });
    input
}

fn file_write_action(approval_status: AgentApprovalStatus) -> AgentProposedAction {
    AgentProposedAction::FileWrite {
        file_write: AgentFileWriteProposal {
            id: "write-1".to_string(),
            draft_id: "draft-1".to_string(),
            mode: AgentFileWriteMode::Create,
            file_path: "report.txt".to_string(),
            base_revision: None,
            summary: None,
            additions: 1,
            deletions: 0,
            line_count: 1,
            byte_count: 5,
            approval_status,
        },
    }
}

#[test]
fn automatic_host_writes_require_auto_approve_and_an_approved_snapshot() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert!(authorize_structured_file_write(
        &manual,
        &file_write_action(AgentApprovalStatus::Approved),
        FileWriteAuthorizationSource::Automatic,
    )
    .is_err());

    let automatic = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert!(authorize_structured_file_write(
        &automatic,
        &file_write_action(AgentApprovalStatus::Approved),
        FileWriteAuthorizationSource::Automatic,
    )
    .is_ok());
    assert!(authorize_structured_file_write(
        &automatic,
        &file_write_action(AgentApprovalStatus::Required),
        FileWriteAuthorizationSource::Automatic,
    )
    .is_err());
}

#[test]
fn explicit_user_approval_authorizes_manual_writes_but_never_overrides_write_denied() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert!(authorize_structured_file_write(
        &manual,
        &file_write_action(AgentApprovalStatus::Required),
        FileWriteAuthorizationSource::ExplicitUser,
    )
    .is_ok());

    let denied = test_input(AgentPermissions {
        write: AgentWritePermission::Denied,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert!(authorize_structured_file_write(
        &denied,
        &file_write_action(AgentApprovalStatus::Approved),
        FileWriteAuthorizationSource::Automatic,
    )
    .is_err());
    assert!(authorize_structured_file_write(
        &denied,
        &file_write_action(AgentApprovalStatus::Required),
        FileWriteAuthorizationSource::ExplicitUser,
    )
    .is_err());
}

#[test]
fn process_actions_remain_in_their_separate_command_policy_domain() {
    let denied = test_input(AgentPermissions::default());
    let command = AgentProposedAction::Command {
        command: command_request("command-1", "pwd"),
    };

    assert!(authorize_structured_file_write(
        &denied,
        &command,
        FileWriteAuthorizationSource::Automatic,
    )
    .is_ok());
}
