#[cfg(test)]
mod pending_action_identity_tests {
    use super::*;

    fn pending_record(action_id: &str, run_id: &str) -> AgentPendingActionRecord {
        AgentPendingActionRecord {
            action_id: action_id.to_string(),
            run_id: run_id.to_string(),
            conversation_id: Some("conversation-1".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
            action_type: "file_change".to_string(),
            tool_name: "apply_patch".to_string(),
            tool_call_id: Some("call-1".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn renderer_action_id_requires_exact_current_canonical_framing() {
        let run_id = "run:1";
        let canonical = crate::canonical_pending_action_id(run_id, "call:1");
        let current = pending_record(&canonical, run_id);
        assert_eq!(
            renderer_action_id_from_pending_record(&current).unwrap(),
            "call:1"
        );

        for malformed in [
            "call:1".to_string(),
            "v2:3:run:call:1".to_string(),
            "v2:5:run:2:".to_string(),
        ] {
            assert_eq!(
                renderer_action_id_from_pending_record(&pending_record(&malformed, run_id)),
                Err("pending action durable identity is malformed".to_string())
            );
        }
    }
}

#[cfg(test)]
mod manual_command_handoff_projection_tests {
    use super::*;

    const SESSION_ID: &str = "cmd_0123456789abcdef0123456789abcdef";

    fn valid_tool_result_value() -> serde_json::Value {
        serde_json::json!({
            "status": "running",
            "sessionId": SESSION_ID,
            "output": "started\n",
            "startedAt": 1,
            "latestSequence": 2,
            "outputTruncated": false,
            "continueWith": {
                "tool": "command_session",
                "args": {
                    "sessionId": SESSION_ID,
                    "action": "wait"
                }
            }
        })
    }

    fn tool_result(result: serde_json::Value) -> AgentToolResult {
        AgentToolResult {
            call_id: "call-1".to_string(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(result),
            error: None,
            exact_archive_file: None,
        }
    }

    #[test]
    fn accepts_exact_command_session_wait_continuation() {
        validate_manual_command_handoff_projection(&tool_result(valid_tool_result_value()))
            .expect("canonical running handoff must validate");
    }

    #[test]
    fn rejects_missing_or_noncanonical_command_session_wait_continuation() {
        let mut invalid_results = Vec::new();

        let mut missing = valid_tool_result_value();
        missing.as_object_mut().unwrap().remove("continueWith");
        invalid_results.push(missing);

        let mut extra_result_field = valid_tool_result_value();
        extra_result_field["unexpected"] = serde_json::json!(true);
        invalid_results.push(extra_result_field);

        let mut wrong_tool = valid_tool_result_value();
        wrong_tool["continueWith"]["tool"] = serde_json::json!("run_command");
        invalid_results.push(wrong_tool);

        let mut extra_continuation_field = valid_tool_result_value();
        extra_continuation_field["continueWith"]["unexpected"] = serde_json::json!(true);
        invalid_results.push(extra_continuation_field);

        let mut wrong_action = valid_tool_result_value();
        wrong_action["continueWith"]["args"]["action"] = serde_json::json!("poll");
        invalid_results.push(wrong_action);

        let mut mismatched_session = valid_tool_result_value();
        mismatched_session["continueWith"]["args"]["sessionId"] =
            serde_json::json!("cmd_ffffffffffffffffffffffffffffffff");
        invalid_results.push(mismatched_session);

        let mut extra_arg = valid_tool_result_value();
        extra_arg["continueWith"]["args"]["unexpected"] = serde_json::json!(true);
        invalid_results.push(extra_arg);

        let mut malformed_args = valid_tool_result_value();
        malformed_args["continueWith"]["args"] = serde_json::json!(null);
        invalid_results.push(malformed_args);

        for result in invalid_results {
            assert_eq!(
                validate_manual_command_handoff_projection(&tool_result(result)),
                Err("manual command handoff ToolResult is invalid".to_string())
            );
        }
    }
}
