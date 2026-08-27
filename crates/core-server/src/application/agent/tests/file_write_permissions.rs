use super::*;
use mycopilot_core::{AgentDiffProposal, AgentPatchOperation};

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

fn apply_patch_action(approval_status: AgentApprovalStatus) -> AgentProposedAction {
    AgentProposedAction::Diff {
        diff: AgentDiffProposal {
            id: "patch-1".to_string(),
            operation: AgentPatchOperation::Create,
            file_path: "report.txt".to_string(),
            patch: "*** Begin Patch\n*** Add File: report.txt\n+hello\n*** End Patch\n".to_string(),
            base_revision: None,
            summary: None,
            approval_status,
        },
    }
}

fn structured_file_write_actions(approval_status: AgentApprovalStatus) -> [AgentProposedAction; 2] {
    [
        apply_patch_action(approval_status),
        file_write_action(approval_status),
    ]
}

fn assert_structured_file_write_authorization(
    input: &AgentChatInput,
    approval_status: AgentApprovalStatus,
    source: FileWriteAuthorizationSource,
    allowed: bool,
) {
    for action in structured_file_write_actions(approval_status) {
        assert_eq!(
            authorize_structured_file_write(input, &action, source).is_ok(),
            allowed
        );
    }
}

#[test]
fn automatic_host_writes_require_auto_approve_and_an_approved_snapshot() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &manual,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        false,
    );

    let automatic = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &automatic,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        true,
    );
    assert_structured_file_write_authorization(
        &automatic,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::Automatic,
        false,
    );
}

#[test]
fn explicit_user_approval_authorizes_manual_writes_but_never_overrides_write_denied() {
    let manual = test_input(AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        patch: mycopilot_core::AgentPatchPermission::RequireApproval,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &manual,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::ExplicitUser,
        true,
    );

    let denied = test_input(AgentPermissions {
        write: AgentWritePermission::Denied,
        patch: mycopilot_core::AgentPatchPermission::AutoApprove,
        ..Default::default()
    });
    assert_structured_file_write_authorization(
        &denied,
        AgentApprovalStatus::Approved,
        FileWriteAuthorizationSource::Automatic,
        false,
    );
    assert_structured_file_write_authorization(
        &denied,
        AgentApprovalStatus::Required,
        FileWriteAuthorizationSource::ExplicitUser,
        false,
    );
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
