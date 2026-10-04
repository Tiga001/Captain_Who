#[test]
fn fork_rejects_terminal_apply_patch_audit_bound_to_another_trace_run() {
    let fixture = current_apply_patch_staged_fork_fixture("draft\n", "after\n");
    let source_audit_id = crate::canonical_pending_action_id("run-source-1", COMMIT_CALL_ID);
    let audit = agent_action_audit_repository::load_action_audit_record(
        &fixture.connection,
        &source_audit_id,
    )
    .unwrap()
    .unwrap();
    let mut action: AgentProposedAction = serde_json::from_str(&audit.action_json).unwrap();
    let AgentProposedAction::FileChange { file_change } = &mut action else {
        unreachable!()
    };
    // Keep the audit and frozen action internally consistent, including a valid canonical
    // action ID, but assign them to another real run. The unchanged call/digest belong only
    // to assistant-b's run and must not be accepted as assistant-a's execution history.
    file_change.execution.run_id = "run-source-0".to_string();
    file_change.execution.observation.run_id = "run-source-0".to_string();
    file_change.validate().unwrap();
    fixture
        .connection
        .execute(
            "UPDATE agent_action_audit
             SET action_id=?1, run_id='run-source-0', assistant_message_id='assistant-a',
                 action_json=?2
             WHERE action_id=?3",
            params![
                crate::canonical_pending_action_id("run-source-0", COMMIT_CALL_ID),
                serde_json::to_string(&action).unwrap(),
                source_audit_id,
            ],
        )
        .unwrap();
    assert_current_staged_fork_rejected(&fixture, "文件修改审计的 Tool Call 所属运行与历史不一致");
}
