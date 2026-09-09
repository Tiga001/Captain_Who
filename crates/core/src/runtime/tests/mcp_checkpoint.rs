use super::*;
use crate::{
    mcp_normalized_input_schema_identity, AgentCollaborationCaller,
    AgentCollaborationRuntimeServices, AgentCollaborationSelectorDirectory,
    AgentCollaborationSettings, AgentMcpApprovalMode, AgentMcpServerScope, AgentMcpToolApproval,
    AgentMcpToolInvocationIdentity, AgentMcpToolProvenance, FrozenAgentCollaborationPolicySource,
    McpAgentToolDescriptor, McpToolApprovalRequest, McpToolCatalogContext, McpToolInvoker,
    McpToolRuntime, AGENT_COLLABORATION_TOOL_NAMES,
};
use std::sync::Mutex;

const MCP_TOOL: &str = "mcp__checkpoint_fixture__read";

struct CheckpointMcpInvoker;

impl McpToolInvoker for CheckpointMcpInvoker {
    fn catalog(&self, _: &McpToolCatalogContext) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        let input_schema = json!({"type":"object","properties":{},"additionalProperties":false});
        let normalized = mcp_normalized_input_schema_identity(MCP_TOOL, &input_schema)?;
        Ok(vec![McpAgentToolDescriptor {
            provenance: AgentMcpToolProvenance {
                server_id: "4f4763c4-61a8-4a5a-9455-429f701548e1".into(),
                scope: AgentMcpServerScope::User,
                raw_tool_name: "read".into(),
                model_tool_name: MCP_TOOL.into(),
                config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".into(),
                registry_revision: 1,
                config_digest: "a".repeat(64),
                catalog_generation: 1,
                catalog_digest: "b".repeat(64),
                catalog_schema_digest: "c".repeat(64),
                schema_digest: normalized.schema_digest,
                schema_normalizer_version: normalized.normalizer_version,
            },
            approval_mode: AgentMcpApprovalMode::Auto,
            server_display_name: "Checkpoint fixture".into(),
            description: Some("Return a fixed test result".into()),
            input_schema,
            output_schema: None,
            annotations: Default::default(),
        }])
    }

    fn prepare_approval(
        &self,
        request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        let (approval, arguments, _) = request.into_parts();
        assert_eq!(arguments, json!({}));
        Ok(approval)
    }

    fn invalidate_prepared_approval(&self, _: &AgentMcpToolInvocationIdentity) -> AgentResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn disabled_collaboration_does_not_block_the_automatic_mcp_checkpoint() {
    struct UnusedCollaboration;
    impl crate::AgentCollaborationExecutor for UnusedCollaboration {
        fn execute(
            &self,
            _: crate::AgentCollaborationInvocation,
            _: crate::AgentCollaborationExecutionControl,
        ) -> crate::AgentCollaborationExecutionFuture {
            panic!("disabled collaboration must not execute")
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for response in [
            json!({"choices":[{"message":{"role":"assistant","content":null,
                "tool_calls":[{"id":"mcp-call","type":"function",
                    "function":{"name":MCP_TOOL,"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[{"message":{"role":"assistant","content":"MCP completed"},"finish_reason":"stop"}]}),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            captured.push(read_runtime_test_json_request(&mut stream).await);
            write_runtime_test_json_response(&mut stream, response).await;
        }
        captured
    });
    let mut input = conversation_context_input(vec![message("user", "Use the test MCP tool")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".into();
    input.stream = Some(false);
    freeze_runtime_test_generic_provider(&mut input, "disabled-collaboration-mcp");

    let captured_checkpoint = Arc::new(Mutex::new(None::<crate::AgentRunCheckpoint>));
    let checkpoint_at_dispatch = Arc::clone(&captured_checkpoint);
    let executor: AgentHostActionExecutor = Arc::new(move |action, checkpoint, cancellation| {
        assert!(!cancellation.is_cancelled());
        let AgentProposedAction::McpToolCall { approval } = action else {
            panic!("expected MCP automatic dispatch")
        };
        let checkpoint = checkpoint.expect("automatic MCP dispatch freezes a checkpoint");
        assert!(checkpoint.collaboration_run_snapshot.is_none());
        assert_eq!(
            checkpoint.pending_action_id.as_deref(),
            Some(approval.identity.action_id.as_str())
        );
        assert!(checkpoint
            .tool_set
            .exposed_tool_names
            .iter()
            .any(|name| name == MCP_TOOL));
        assert!(!checkpoint
            .tool_set
            .exposed_tool_names
            .iter()
            .any(|name| AGENT_COLLABORATION_TOOL_NAMES.contains(&name.as_str())));
        assert!(
            checkpoint_at_dispatch
                .lock()
                .unwrap()
                .replace(checkpoint)
                .is_none(),
            "dispatch must occur once"
        );
        Ok(AgentToolResult {
            call_id: approval.identity.call_id,
            tool: MCP_TOOL.into(),
            ok: true,
            result: Some(json!({"status":"completed","value":"fixture result"})),
            error: None,
            exact_archive_file: None,
        })
    });
    let fixture = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("core.sqlite")).unwrap());
    let host_services = AgentRuntimeHostServices::new()
        .with_host_actions(executor, storage)
        .with_mcp_tools(McpToolRuntime::capture(Arc::new(CheckpointMcpInvoker)))
        // Embedded Hosts may offer the port even while their frozen policy disables it.
        .with_agent_collaboration(AgentCollaborationRuntimeServices::new(
            Arc::new(UnusedCollaboration),
            AgentCollaborationCaller {
                agent_id: "root".into(),
                root_agent_id: "root".into(),
                root_conversation_id: "mcp-checkpoint".into(),
                parent_agent_id: None,
                conversation_id: "mcp-checkpoint".into(),
                project_id: None,
                task_name: crate::ROOT_AGENT_TASK_NAME.into(),
                task_path: "/root".into(),
            },
            AgentCollaborationSelectorDirectory::default(),
        ))
        .with_agent_collaboration_policy(Arc::new(FrozenAgentCollaborationPolicySource::new(
            AgentCollaborationSettings {
                enabled: false,
                ..Default::default()
            },
        )));
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        AgentRuntime::default().send_chat_with_events_and_cancellation(
            input,
            Some("disabled-collaboration-mcp-run".into()),
            None,
            AgentCancellationToken::new(),
            Some(host_services),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(output.content, "MCP completed");
    assert!(captured_checkpoint.lock().unwrap().is_some());
    assert!(!output
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequired { .. })));
    for request in server.await.unwrap() {
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        assert!(names.contains(&MCP_TOOL));
        assert!(!names
            .iter()
            .any(|name| AGENT_COLLABORATION_TOOL_NAMES.contains(name)));
    }
}
