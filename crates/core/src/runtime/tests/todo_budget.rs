use super::*;
use crate::protocol::{AgentTodoState, AgentTodoStatus};

fn todo_budget_wire_reminder(request: &Value) -> &str {
    let mut reminders = Vec::new();
    for message in request["messages"].as_array().unwrap() {
        if let Some(content) = message["content"].as_str() {
            if content.starts_with("## Runtime todo\n") {
                reminders.push(content);
            }
        }
        if let Some(blocks) = message["content"].as_array() {
            for block in blocks {
                if let Some(text) = block["text"].as_str() {
                    if text.starts_with("## Runtime todo\n") {
                        reminders.push(text);
                    }
                }
            }
        }
    }
    assert_eq!(
        reminders.len(),
        1,
        "Expected exactly the latest Todo reminder"
    );
    reminders[0]
}

fn todo_budget_wire_results(request: &Value, style: crate::AgentApiStyle) -> Vec<Value> {
    let mut results = Vec::new();
    for message in request["messages"].as_array().unwrap() {
        match style {
            crate::AgentApiStyle::OpenAiCompatible if message["role"] == "tool" => {
                results.push(serde_json::from_str(message["content"].as_str().unwrap()).unwrap());
            }
            crate::AgentApiStyle::AnthropicCompatible => {
                if let Some(blocks) = message["content"].as_array() {
                    for block in blocks {
                        if block["type"] == "tool_result" {
                            assert_eq!(block["is_error"], false);
                            results.push(
                                serde_json::from_str(block["content"].as_str().unwrap()).unwrap(),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    results
}

fn assert_todo_budget_wire_state(
    request: &Value,
    style: crate::AgentApiStyle,
    revision: u64,
    item_count: usize,
    status: &str,
) {
    let reminder = todo_budget_wire_reminder(request);
    let budget = crate::context::ContextTextBudget::heuristic(500);
    assert!(
        budget.fits(reminder),
        "{style:?} revision {revision} reminder uses {} tokens:\n{reminder}",
        budget.estimate(reminder)
    );
    assert!(reminder.contains(&format!("revision: {revision}\n")));
    assert_eq!(
        reminder
            .lines()
            .filter(|line| line.starts_with("- ref="))
            .count(),
        item_count
    );
    for reference in 1..=item_count {
        let prefix = format!("- ref={reference} [{status}]");
        assert!(
            reminder.lines().any(|line| line.starts_with(&prefix)),
            "{reminder}"
        );
    }
    assert!(reminder.contains("Titles/notes abbreviated; stored items unchanged."));
    assert!(!reminder.contains("Do not call more tools"));

    let results = todo_budget_wire_results(request, style);
    assert_eq!(results.len(), revision as usize);
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result["accepted"], true, "{style:?}: {result}");
        assert_eq!(result["revision"], index as u64 + 1);
        assert_eq!(result["itemCount"], item_count);
        assert_eq!(
            result["completedCount"],
            if index == 0 { 0 } else { item_count }
        );
        assert!(result.get("error").is_none());
        assert!(result.get("items").is_none());
    }
    assert!(!request
        .to_string()
        .contains("500-token Todo context budget"));
}

fn assert_todo_budget_canonical_items(state: &AgentTodoState, expected: &[Value], completed: bool) {
    assert_eq!(state.items.len(), expected.len());
    for (item, expected) in state.items.iter().zip(expected) {
        assert_eq!(item.id, expected["id"].as_str().unwrap());
        assert_eq!(item.title, expected["title"].as_str().unwrap());
        assert_eq!(item.note.as_deref(), expected["note"].as_str());
        assert_eq!(
            item.status,
            if completed {
                AgentTodoStatus::Completed
            } else {
                AgentTodoStatus::InProgress
            }
        );
        assert!(item.created_at > 0);
        assert!(item.updated_at >= item.created_at);
    }
}

#[tokio::test]
async fn todo_budget_large_chinese_plan_completes_without_retry_for_both_provider_payloads() {
    async fn run_case(style: crate::AgentApiStyle) {
        let titles = [
            "检查待办预算拒绝的执行路径并确认失败原因",
            "将完整待办存储与模型提醒投影的预算限制分离",
            "保留全部待办引用并按预算压缩备注及已完成任务",
            "覆盖中文计划逐步推进与全部完成时的状态更新",
            "运行相关回归测试并检查恢复快照后的状态一致性",
        ];
        let note =
            "已确认原有固定说明和完成提示导致提醒继续增长，需要完整保留每个任务的进度与备注。";
        let items = titles
            .iter()
            .enumerate()
            .map(|(index, title)| {
                json!({
                    "id": format!("中文稳定标识{}", index + 1),
                    "title": title,
                    "status": "in_progress",
                    "note": note
                })
            })
            .collect::<Vec<_>>();
        let full_plan_text = items
            .iter()
            .map(|item| {
                format!(
                    "{} {} {}",
                    item["id"].as_str().unwrap(),
                    item["title"].as_str().unwrap(),
                    item["note"].as_str().unwrap()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(crate::context::ContextTextBudget::heuristic(500).estimate(&full_plan_text) > 609);
        let create_args = json!({ "items": items });
        let complete_args = json!({
            "expectedRevision": 1,
            "items": (1..=items.len()).map(|reference| json!({
                "ref": reference,
                "status": "completed"
            })).collect::<Vec<_>>()
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for index in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_runtime_test_json_request(&mut stream).await);
                let args = if index == 0 {
                    &create_args
                } else {
                    &complete_args
                };
                let call_id = format!("todo-budget-{index}");
                let response = match (style, index) {
                    (crate::AgentApiStyle::OpenAiCompatible, 0 | 1) => json!({
                        "choices": [{ "message": {
                            "role": "assistant",
                            "content": "更新计划进度。",
                            "tool_calls": [{ "id": call_id, "type": "function", "function": {
                                "name": "todo_update", "arguments": args.to_string()
                            }}]
                        }, "finish_reason": "tool_calls" }]
                    }),
                    (crate::AgentApiStyle::AnthropicCompatible, 0 | 1) => json!({
                        "content": [
                            { "type": "text", "text": "更新计划进度。" },
                            { "type": "tool_use", "id": call_id, "name": "todo_update", "input": args }
                        ],
                        "stop_reason": "tool_use"
                    }),
                    (crate::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{ "message": { "role": "assistant", "content": "预算回归完成。" }, "finish_reason": "stop" }]
                    }),
                    (crate::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "预算回归完成。" }], "stop_reason": "end_turn"
                    }),
                };
                write_runtime_test_json_response(&mut stream, response).await;
            }
            requests
        });

        let mut input =
            conversation_context_input(vec![message("user", "创建中文计划并完成全部事项。")]);
        input.api_url = match style {
            crate::AgentApiStyle::OpenAiCompatible => {
                format!("http://{address}/v1/chat/completions")
            }
            crate::AgentApiStyle::AnthropicCompatible => format!("http://{address}/v1/messages"),
        };
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        input.stream = Some(false);
        freeze_runtime_test_generic_provider(&mut input, "todo-budget-round-trip");
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AgentRuntime::default().send_chat_with_events_and_cancellation(
                input,
                Some("todo-budget-round-trip".to_string()),
                None,
                AgentCancellationToken::new(),
                None,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(output.content, "预算回归完成。");
        assert_eq!(output.status, crate::protocol::AgentRunStatus::Completed);
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(!requests[0]["messages"]
            .to_string()
            .contains("## Runtime todo"));
        assert_todo_budget_wire_state(&requests[1], style, 1, items.len(), "in_progress");
        assert_todo_budget_wire_state(&requests[2], style, 2, items.len(), "completed");

        let tool_calls = output
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolCall { call, .. } => Some(call),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_calls.len(), 2, "No extra tools or retries are needed");
        assert!(tool_calls.iter().all(|call| call.tool == "todo_update"));
        let tool_results = output
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolResult { result, .. } => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_results.len(), 2);
        assert!(tool_results
            .iter()
            .all(|result| result.tool == "todo_update" && result.ok && result.error.is_none()));
        let todo_updates = output
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TodoUpdated { run_id, todo } => {
                    assert_eq!(run_id, &output.run_id);
                    Some(todo)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(todo_updates.len(), 2);
        for (index, state) in todo_updates.iter().enumerate() {
            assert_eq!(state.revision, index as u64 + 1);
            assert_todo_budget_canonical_items(state, &items, index == 1);
            assert_eq!(
                tool_results[index].result.as_ref().unwrap(),
                &serde_json::to_value(state).unwrap(),
                "Renderer ToolResult and TodoUpdated retain the same full state"
            );
            assert_eq!(tool_results[index].call_id, tool_calls[index].id);
        }
        for (before, after) in todo_updates[0].items.iter().zip(&todo_updates[1].items) {
            assert_eq!(after.created_at, before.created_at);
        }
        let final_todo = output.todo.as_ref().unwrap();
        assert_eq!(final_todo.revision, 2);
        assert_todo_budget_canonical_items(final_todo, &items, true);
        assert_eq!(
            serde_json::to_value(final_todo).unwrap(),
            serde_json::to_value(todo_updates[1]).unwrap()
        );
    }

    run_case(crate::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::AgentApiStyle::AnthropicCompatible).await;
}
