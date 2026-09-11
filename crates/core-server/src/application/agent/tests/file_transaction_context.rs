//! FileChange context regressions through the real Host, Harness and a controlled HTTP Provider.
use super::human_input::{read_provider_request, wait_for_done};
use super::*;
use mycopilot_core::{
    AgentCollaborationSettingsUpdate, AgentCommandPermission, AgentCommandSafetyPolicy,
    AgentContextProfile, AgentPatchPermission, AGENT_COLLABORATION_TOOL_NAMES,
};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const STATE_HEADER: &str = "Backend file transaction state.";
const ASSISTANT_ID: &str = "assistant-file-transaction-context";
const ORIGINAL_INPUT: &str = "Build the requested files and retain the ordinary tool history.";
const DRAFT_CONTENT: &str = "private-staged-body-context-9217\n";
const SUPPRESSED_TEXT: &str = "premature-file-transaction-progress-9217";
const SUPPRESSED_TOOL_TEXT: &str = "hidden-append-progress-9217";
const CORRECTION_HEADER: &str = "Your preceding text-only response was not shown";

struct FileTransactionHost {
    _fixture: tempfile::TempDir,
    workspace: std::path::PathBuf,
    storage: Arc<StorageService>,
    agent: AgentService,
    run_id: String,
    requests: UnboundedReceiver<Value>,
    replies: UnboundedSender<Value>,
    notifications: UnboundedSender<Value>,
    events: UnboundedReceiver<Value>,
    provider: tokio::task::JoinHandle<()>,
}

impl FileTransactionHost {
    async fn start(request_count: usize, patch: AgentPatchPermission) -> Self {
        Self::start_with_collaboration(request_count, patch, AgentContextProfile::Full, true).await
    }

    async fn start_with_collaboration(
        request_count: usize,
        patch: AgentPatchPermission,
        context_profile: AgentContextProfile,
        collaboration_enabled: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (captured, requests) = unbounded_channel();
        let (replies, mut response_deltas) = unbounded_channel::<Value>();
        let provider = tokio::spawn(async move {
            for _ in 0..request_count {
                let (mut stream, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .expect("Host must issue the next Provider request")
                        .unwrap();
                captured
                    .send(read_provider_request(&mut stream).await)
                    .unwrap();
                let delta = tokio::time::timeout(Duration::from_secs(10), response_deltas.recv())
                    .await
                    .expect("test must release each Provider response")
                    .expect("test response channel closed");
                let finish_reason = if delta.get("tool_calls").is_some() {
                    "tool_calls"
                } else {
                    "stop"
                };
                let content = json!({"choices":[{"delta":delta,"finish_reason":null}]});
                let finish = json!({"choices":[{"delta":{},"finish_reason":finish_reason}]});
                stream.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {content}\n\ndata: {finish}\n\ndata: [DONE]\n\n"
                ).as_bytes()).await.unwrap();
            }
        });

        let fixture = tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
        storage
            .save_project(ProjectRecord::with_primary_folder(
                "project-file-transaction-context".to_string(),
                "File transaction context integration".to_string(),
                workspace.to_string_lossy().into_owned(),
                1,
            ))
            .unwrap();
        let mut settings = test_model_settings();
        settings.api_url = format!("http://{address}/v1/chat/completions");
        storage.save_model_settings(settings).unwrap();
        let mut preferences = storage.load_agent_prompt_preferences().unwrap();
        preferences.context_profile = context_profile;
        storage.save_agent_prompt_preferences(preferences).unwrap();
        set_collaboration_enabled(&storage, collaboration_enabled);
        let agent = AgentService::new(Arc::clone(&storage));
        let (notifications, events) = unbounded_channel();
        let turn = agent
            .start_conversation_turn(
                AgentConversationTurnInput {
                    conversation_id: Some("conversation-file-transaction-context".to_string()),
                    project_id: Some("project-file-transaction-context".to_string()),
                    model_id: "model-1".to_string(),
                    context_window_indicator_enabled: false,
                    content: ORIGINAL_INPUT.to_string(),
                    attachments: Vec::new(),
                    skills: Vec::new(),
                    title: None,
                    user_message_id: Some("user-file-transaction-context".to_string()),
                    assistant_message_id: Some(ASSISTANT_ID.to_string()),
                    max_tokens: None,
                    temperature: None,
                    prompt_preferences: None,
                    permissions: AgentPermissions {
                        write: AgentWritePermission::WorkspaceOnly,
                        patch,
                        command: AgentCommandPermission::AutoApprove,
                        command_safety: AgentCommandSafetyPolicy::FullAccess,
                        ..AgentPermissions::default()
                    },
                },
                notifications.clone(),
            )
            .unwrap();
        Self {
            _fixture: fixture,
            workspace,
            storage,
            agent,
            run_id: turn.run_id,
            requests,
            replies,
            notifications,
            events,
            provider,
        }
    }

    async fn request(&mut self) -> Value {
        match tokio::time::timeout(Duration::from_secs(10), self.requests.recv()).await {
            Ok(Some(request)) => request,
            failure => {
                let captured_events = std::iter::from_fn(|| self.events.try_recv().ok())
                    .map(|event| {
                        let params = &event["params"];
                        json!({
                            "method": event["method"], "type": params["type"],
                            "status": params["status"], "code": params["code"],
                            "message": params["message"], "error": params["error"],
                        })
                    })
                    .collect::<Vec<_>>();
                let terminal = self
                    .storage
                    .get_conversation_turn_trace(ASSISTANT_ID)
                    .map(|trace| trace.map(|trace| (trace.terminal_status, trace.terminal_error)));
                panic!(
                    "Host must reach the next controlled Provider request; wait={failure:?}; \
                     run={}; terminal={terminal:?}; events={captured_events:?}",
                    self.run_id,
                );
            }
        }
    }

    fn calls(&self, calls: &[(&str, &str, Value)]) {
        self.calls_with_text(calls, None);
    }

    fn calls_with_text(&self, calls: &[(&str, &str, Value)], content: Option<&str>) {
        self.replies
            .send(json!({
                "role": "assistant",
                "content": content,
                "tool_calls": calls.iter().enumerate().map(|(index, (id, name, args))| json!({
                    "index": index,
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}
                })).collect::<Vec<_>>()
            }))
            .unwrap();
    }

    fn patch(&self, id: &str, request: Value) {
        self.calls(&[(id, "apply_patch", json!({"request": request}))]);
    }

    fn text(&self, content: &str) {
        self.replies
            .send(json!({"role":"assistant", "content":content}))
            .unwrap();
    }

    fn call_id(&self, provider_id: &str) -> String {
        let log = self
            .storage
            .get_conversation_model_context_log(ASSISTANT_ID)
            .unwrap()
            .unwrap();
        let calls = log
            .items
            .iter()
            .flat_map(|item| &item.tool_calls)
            .filter(|call| call.provider_identity.provider_call_id == provider_id)
            .collect::<Vec<_>>();
        assert_eq!(
            calls.len(),
            1,
            "one durable canonical identity for {provider_id}"
        );
        calls[0].id.clone()
    }

    async fn finish(mut self) {
        self.text("The requested file transaction work is complete.");
        wait_for_done(&mut self.events, &self.run_id, "completed").await;
        self.provider.await.unwrap();
        let log = self
            .storage
            .get_conversation_model_context_log(ASSISTANT_ID)
            .unwrap()
            .unwrap();
        assert!(
            log.items
                .iter()
                .all(|item| !item.content.contains(STATE_HEADER)),
            "the dynamic transaction tail must never become retained model history"
        );
    }
}

fn set_collaboration_enabled(storage: &StorageService, enabled: bool) {
    let current = storage.load_agent_collaboration_settings().unwrap();
    if current.enabled != enabled {
        storage
            .update_agent_collaboration_settings(&AgentCollaborationSettingsUpdate {
                enabled,
                expected_revision: current.revision,
            })
            .unwrap();
    }
}

fn assert_collaboration_request(request: &Value, enabled: bool, profile: AgentContextProfile) {
    let tools = request["tools"].as_array().unwrap();
    for name in AGENT_COLLABORATION_TOOL_NAMES {
        assert_eq!(
            tools
                .iter()
                .filter(|tool| tool["function"]["name"] == name)
                .count(),
            usize::from(enabled),
            "the actual Provider request must obey the frozen collaboration policy: {name}"
        );
    }
    assert_eq!(
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == "todo_update"),
        profile == AgentContextProfile::Full,
        "the real run must exercise the requested context profile"
    );
    let text = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text.contains("<agent_collaboration_directory>"), enabled);
    if !enabled {
        assert!(
            text.contains("disabled_by_user"),
            "the disabled capability must retain its World State explanation"
        );
    }
}

#[tokio::test]
async fn collaboration_file_change_disabled_auto_create_succeeds_in_full_and_minimal_modes() {
    for profile in [AgentContextProfile::Full, AgentContextProfile::Minimal] {
        let mut host = FileTransactionHost::start_with_collaboration(
            2,
            AgentPatchPermission::AutoApprove,
            profile,
            false,
        )
        .await;
        assert_collaboration_request(&host.request().await, false, profile);
        host.patch(
            "collaboration-disabled-create",
            json!({
                "action": "apply", "operation": "create", "filePath": "quick_sort.py",
                "content": "def quick_sort(values):\n    return sorted(values)\n"
            }),
        );
        let after_create = host.request().await;
        assert_collaboration_request(&after_create, false, profile);
        assert_eq!(
            tool_result(&host, &after_create, "collaboration-disabled-create")["status"],
            "applied",
            "disabled collaboration must not invalidate an unrelated FileChange checkpoint"
        );
        assert_eq!(
            fs::read_to_string(host.workspace.join("quick_sort.py")).unwrap(),
            "def quick_sort(values):\n    return sorted(values)\n"
        );
        assert!(host.agent.list_pending_actions().is_empty());
        assert!(
            !host
                .storage
                .load_agent_collaboration_run_policy(&host.run_id)
                .unwrap()
                .unwrap()
                .enabled
        );
        host.finish().await;
    }
}

#[tokio::test]
async fn collaboration_file_change_approval_and_preview_preserve_frozen_policy_after_switch() {
    for profile in [AgentContextProfile::Full, AgentContextProfile::Minimal] {
        for enabled in [false, true] {
            let mut host = FileTransactionHost::start_with_collaboration(
                2,
                AgentPatchPermission::RequireApproval,
                profile,
                enabled,
            )
            .await;
            assert_collaboration_request(&host.request().await, enabled, profile);
            host.patch(
                "collaboration-approval-create",
                json!({
                    "action": "apply", "operation": "create", "filePath": "approved.py",
                    "content": "print('approved once')\n"
                }),
            );
            wait_for_done(&mut host.events, &host.run_id, "waiting_for_approval").await;
            assert!(!host.workspace.join("approved.py").exists());
            let pending = host.agent.list_pending_actions();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].tool_name, "apply_patch");
            let frozen_input = host
                .agent
                .pending_actions
                .lock()
                .unwrap()
                .values()
                .find(|record| record.snapshot.run_id == host.run_id)
                .unwrap()
                .agent_input
                .clone();
            let checkpoint = frozen_input.resume_checkpoint.as_ref().unwrap();
            assert_eq!(checkpoint.collaboration_run_snapshot.is_some(), enabled);
            assert_eq!(
                frozen_input
                    .prompt_preferences
                    .as_ref()
                    .unwrap()
                    .context_profile,
                profile
            );
            set_collaboration_enabled(&host.storage, !enabled);
            assert_eq!(
                host.storage
                    .load_agent_collaboration_settings()
                    .unwrap()
                    .enabled,
                !enabled
            );
            assert_eq!(
                host.storage
                    .load_agent_collaboration_run_policy(&host.run_id)
                    .unwrap()
                    .unwrap()
                    .enabled,
                enabled
            );

            // Reconstruct both preview routes after discarding the cached selector directory.
            // A disabled run has no authorization snapshot even if the global switch is now on.
            host.agent
                .collaboration_run_directories
                .lock()
                .unwrap()
                .clear();
            for projection in [
                host.agent
                    .context_window_tool_projection(&frozen_input, None),
                host.agent.context_window_tool_projection_for_agent_run(
                    &host.run_id,
                    &frozen_input,
                    None,
                ),
            ] {
                let projected = projection.unwrap().tool_set_checkpoint();
                assert_eq!(
                    serde_json::to_value(projected).unwrap(),
                    serde_json::to_value(&checkpoint.tool_set).unwrap(),
                    "preview must reconstruct exactly the approved run's frozen Tool set"
                );
            }

            host.agent
                .approve_action(
                    &host.run_id,
                    &pending[0].action_id,
                    host.notifications.clone(),
                )
                .unwrap();
            let resumed = host.request().await;
            assert_collaboration_request(&resumed, enabled, profile);
            assert_eq!(
                tool_result(&host, &resumed, "collaboration-approval-create")["status"],
                "applied"
            );
            assert_eq!(
                fs::read_to_string(host.workspace.join("approved.py")).unwrap(),
                "print('approved once')\n"
            );
            host.finish().await;
        }
    }
}

fn transaction_tail(request: &Value) -> Option<Value> {
    let tails = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter(|content| content.contains(STATE_HEADER))
        .collect::<Vec<_>>();
    assert!(
        tails.len() <= 1,
        "one fresh transaction tail per request: {tails:?}"
    );
    let content = tails.first()?;
    assert!(!content.contains("While userVisibleTextBlocked=true"));
    assert!(!content.contains("Never guess a cursor"));
    assert!(
        !content.contains(DRAFT_CONTENT),
        "the tail must contain metadata only"
    );
    let json_start = content
        .find('{')
        .expect("transaction tail contains JSON metadata");
    assert_eq!(
        content[..json_start].trim(),
        format!("{STATE_HEADER}\n```json"),
        "the tail must not repeat fixed usage instructions"
    );
    let json_end = content.rfind('}').unwrap();
    let payload: Value = serde_json::from_str(&content[json_start..=json_end]).unwrap();
    assert_eq!(
        payload.as_object().unwrap().len(),
        1,
        "only transaction metadata belongs in the tail"
    );
    assert!(
        payload.get("settlements").is_none(),
        "audit settlements cannot be replayed in the tail"
    );
    let transactions = payload["transactions"].as_array().unwrap();
    assert!(!transactions.is_empty(), "empty tails must be omitted");
    for transaction in transactions {
        assert!(
            matches!(
                transaction["status"].as_str(),
                Some("drafting" | "ready" | "waiting_approval" | "applying" | "outcome_unknown")
            ),
            "terminal transaction leaked into the request tail: {transaction}"
        );
        for field in [
            "content",
            "baseContent",
            "summary",
            "result",
            "receipt",
            "observationId",
            "fileChangeTarget",
        ] {
            assert!(
                transaction.get(field).is_none(),
                "nonessential {field} leaked into the tail"
            );
        }
    }
    Some(payload)
}

fn assert_cursor(request: &Value, revision: u64) -> String {
    let tail =
        transaction_tail(request).expect("an unresolved staged draft needs current metadata");
    let transactions = tail["transactions"].as_array().unwrap();
    assert_eq!(
        transactions.len(),
        1,
        "only the unresolved staged transaction enters the tail"
    );
    let transaction = &transactions[0];
    assert!(
        transaction.get("draftRevision").is_none(),
        "avoid duplicating the mutation revision"
    );
    assert_eq!(transaction["expectedDraftRevision"], revision);
    assert_eq!(transaction["nextIndex"], revision);
    assert!(transaction["allowedNextActions"]
        .as_array()
        .unwrap()
        .contains(&json!("abort")));
    transaction["transactionId"].as_str().unwrap().to_string()
}

fn tool_result(host: &FileTransactionHost, request: &Value, provider_id: &str) -> Value {
    let id = host.call_id(provider_id);
    let results = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .collect::<Vec<_>>();
    assert_eq!(
        results.len(),
        1,
        "exactly one ordinary ToolResult for {provider_id} ({id})"
    );
    serde_json::from_str(results[0]["content"].as_str().unwrap()).unwrap()
}

fn assert_call(
    host: &FileTransactionHost,
    request: &Value,
    provider_id: &str,
    name: &str,
    args: &Value,
) {
    let id = host.call_id(provider_id);
    let calls = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flatten()
        .filter(|call| call["id"] == id)
        .collect::<Vec<_>>();
    assert_eq!(
        calls.len(),
        1,
        "ordinary ToolCall {id} must remain exactly once"
    );
    assert_eq!(calls[0]["function"]["name"], name);
    assert_eq!(
        serde_json::from_str::<Value>(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap(),
        *args
    );
}

#[tokio::test]
async fn file_transaction_context_direct_and_committed_rows_leave_no_tail_but_keep_tool_history() {
    let mut host = FileTransactionHost::start(7, AgentPatchPermission::AutoApprove).await;
    assert!(transaction_tail(&host.request().await).is_none());
    let direct = json!({"request": {"action":"apply", "operation":"create", "filePath":"direct.txt", "content":"direct-history-9217\n"}});
    host.calls(&[("context-direct", "apply_patch", direct.clone())]);
    let after_direct = host.request().await;
    assert!(
        transaction_tail(&after_direct).is_none(),
        "Direct-only work adds no transaction tail"
    );
    assert_eq!(
        tool_result(&host, &after_direct, "context-direct")["status"],
        "applied"
    );

    let begin =
        json!({"request":{"action":"begin", "operation":"create", "filePath":"staged.txt"}});
    host.calls(&[("context-begin", "apply_patch", begin.clone())]);
    let after_begin = host.request().await;
    let transaction_id = assert_cursor(&after_begin, 0);
    let append = json!({"request":{"action":"append", "transactionId":transaction_id, "index":0, "expectedDraftRevision":0, "content":DRAFT_CONTENT}});
    host.calls(&[("context-append", "apply_patch", append.clone())]);
    let after_append = host.request().await;
    assert_eq!(assert_cursor(&after_append, 1), transaction_id);
    let edit = json!({"request":{"action":"edit", "transactionId":transaction_id, "index":1, "expectedDraftRevision":1, "edits":[{"kind":"append", "text":"edited-history-9217\n"}]}});
    host.calls(&[("context-edit", "apply_patch", edit.clone())]);
    let after_edit = host.request().await;
    assert_eq!(assert_cursor(&after_edit, 2), transaction_id);
    let commit = json!({"request":{"action":"commit", "transactionId":transaction_id, "expectedDraftRevision":2}});
    host.calls(&[("context-commit", "apply_patch", commit.clone())]);
    let after_commit = host.request().await;
    assert!(
        transaction_tail(&after_commit).is_none(),
        "successful commit removes the dynamic tail"
    );
    assert_eq!(
        tool_result(&host, &after_commit, "context-commit")["status"],
        "applied"
    );
    assert_eq!(
        fs::read_to_string(host.workspace.join("staged.txt")).unwrap(),
        format!("{DRAFT_CONTENT}edited-history-9217\n")
    );
    let read = json!({"path":"direct.txt"});
    host.calls(&[("context-read", "read_file", read.clone())]);
    let final_request = host.request().await;
    assert!(transaction_tail(&final_request).is_none());
    for (id, name, args, original_result) in [
        (
            "context-direct",
            "apply_patch",
            direct,
            tool_result(&host, &after_direct, "context-direct"),
        ),
        (
            "context-begin",
            "apply_patch",
            begin,
            tool_result(&host, &after_begin, "context-begin"),
        ),
        (
            "context-append",
            "apply_patch",
            append,
            tool_result(&host, &after_append, "context-append"),
        ),
        (
            "context-edit",
            "apply_patch",
            edit,
            tool_result(&host, &after_edit, "context-edit"),
        ),
        (
            "context-commit",
            "apply_patch",
            commit,
            tool_result(&host, &after_commit, "context-commit"),
        ),
    ] {
        assert_call(&host, &final_request, id, name, &args);
        assert_eq!(
            tool_result(&host, &final_request, id),
            original_result,
            "ordinary result {id} must survive tail removal unchanged"
        );
    }
    assert_call(&host, &final_request, "context-read", "read_file", &read);
    assert!(tool_result(&host, &final_request, "context-read")
        .to_string()
        .contains("direct-history-9217"));
    host.finish().await;
}

#[tokio::test]
async fn file_transaction_context_same_batch_fence_and_narration_correction_do_not_retain_stale_cursors(
) {
    let mut host = FileTransactionHost::start(5, AgentPatchPermission::AutoApprove).await;
    assert!(transaction_tail(&host.request().await).is_none());
    host.calls(&[
        (
            "context-fence-begin",
            "apply_patch",
            json!({"request":{"action":"begin", "operation":"create", "filePath":"aborted.txt"}}),
        ),
        (
            "context-fence-command",
            "run_command",
            json!({"command":"printf should-not-execute > escaped-fence.txt"}),
        ),
    ]);
    let after_batch = host.request().await;
    let transaction_id = assert_cursor(&after_batch, 0);
    let denied = tool_result(&host, &after_batch, "context-fence-command");
    assert_eq!(
        denied["errorCode"],
        "agent.file_change_transaction_unsettled"
    );
    assert_eq!(denied["type"], "runtime_guard");
    assert!(
        host.storage
            .get_agent_action_audit(&pending_action_storage_id(
                &host.run_id,
                &host.call_id("context-fence-command")
            ))
            .unwrap()
            .is_none(),
        "the fenced command must never enter the privileged action executor"
    );
    assert!(!host.workspace.join("escaped-fence.txt").exists());
    host.text(SUPPRESSED_TEXT);
    let corrected = host.request().await;
    assert_eq!(assert_cursor(&corrected, 0), transaction_id);
    assert!(!corrected["messages"].to_string().contains(SUPPRESSED_TEXT));
    assert_eq!(
        corrected["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["content"]
                .as_str()
                .is_some_and(|content| content.starts_with(CORRECTION_HEADER)))
            .count(),
        1,
        "a suppressed text-only response receives one transient correction"
    );
    host.calls_with_text(&[("context-fence-append", "apply_patch", json!({"request":{"action":"append", "transactionId":transaction_id, "index":0, "expectedDraftRevision":0, "content":DRAFT_CONTENT}}))], Some(SUPPRESSED_TOOL_TEXT));
    let after_append = host.request().await;
    assert_eq!(assert_cursor(&after_append, 1), transaction_id);
    assert!(!after_append["messages"]
        .to_string()
        .contains(SUPPRESSED_TOOL_TEXT));
    assert!(
        !after_append["messages"]
            .to_string()
            .contains(CORRECTION_HEADER),
        "successful tool progress retires transient correction instructions"
    );
    for request in [&corrected, &after_append] {
        for content in request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_str())
            .filter(|content| content.starts_with(CORRECTION_HEADER))
        {
            assert!(
                !content.contains(&transaction_id),
                "retained corrections cannot carry stale transaction identities or cursors"
            );
            assert!(!content.contains("draftRevision"));
            assert!(!content.contains("nextIndex"));
        }
    }
    host.patch(
        "context-fence-abort",
        json!({"action":"abort", "transactionId":transaction_id}),
    );
    let after_abort = host.request().await;
    assert!(
        transaction_tail(&after_abort).is_none(),
        "abort removes the unresolved transaction tail"
    );
    assert_eq!(
        tool_result(&host, &after_abort, "context-fence-abort")["status"],
        "aborted"
    );
    assert!(!host.workspace.join("aborted.txt").exists());
    let trace = host
        .storage
        .get_conversation_turn_trace(ASSISTANT_ID)
        .unwrap()
        .unwrap();
    assert!(!serde_json::to_string(&trace)
        .unwrap()
        .contains(SUPPRESSED_TEXT));
    assert!(!serde_json::to_string(&trace)
        .unwrap()
        .contains(SUPPRESSED_TOOL_TEXT));
    let visibility_events = trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::BackendState { content, .. } => {
                serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(|state| state["type"] == "assistant_text_visibility")
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        visibility_events.len(),
        1,
        "one durable visibility fact for the suppressed tool response"
    );
    let visibility = &visibility_events[0];
    assert_eq!(visibility["status"], "not_shown");
    assert_eq!(visibility["reason"], "file_transaction_unsettled");
    assert!(visibility["modelRequestIndex"].is_u64());
    assert_eq!(visibility.as_object().unwrap().len(), 4);
    assert_eq!(
        host.storage
            .get_agent_file_change(&transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "aborted"
    );
    while let Ok(event) = host.events.try_recv() {
        let serialized = event.to_string();
        assert!(!serialized.contains(SUPPRESSED_TEXT));
        assert!(
            !serialized.contains(SUPPRESSED_TOOL_TEXT),
            "suppressed narration cannot leak through Host notifications"
        );
    }
    host.finish().await;
}

#[tokio::test]
async fn file_transaction_context_approval_resume_uses_the_current_committed_state() {
    let mut host = FileTransactionHost::start(4, AgentPatchPermission::RequireApproval).await;
    assert!(transaction_tail(&host.request().await).is_none());
    host.patch(
        "context-approval-begin",
        json!({"action":"begin", "operation":"create", "filePath":"approved.txt"}),
    );
    let after_begin = host.request().await;
    let transaction_id = assert_cursor(&after_begin, 0);
    host.patch("context-approval-append", json!({"action":"append", "transactionId":transaction_id, "index":0, "expectedDraftRevision":0, "content":DRAFT_CONTENT}));
    let before_approval = host.request().await;
    assert_eq!(assert_cursor(&before_approval, 1), transaction_id);
    let commit = json!({"request":{"action":"commit", "transactionId":transaction_id, "expectedDraftRevision":1}});
    host.calls_with_text(
        &[("context-approval-commit", "apply_patch", commit.clone())],
        Some(SUPPRESSED_TOOL_TEXT),
    );
    wait_for_done(&mut host.events, &host.run_id, "waiting_for_approval").await;
    assert!(!host.workspace.join("approved.txt").exists());
    assert_eq!(
        host.storage
            .get_agent_file_change(&transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "waiting_approval"
    );
    let pending = host.agent.list_pending_actions();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_name, "apply_patch");
    host.agent
        .approve_action(
            &host.run_id,
            &pending[0].action_id,
            host.notifications.clone(),
        )
        .unwrap();
    let resumed = host.request().await;
    assert!(
        transaction_tail(&resumed).is_none(),
        "resume must reload the committed state instead of the pending checkpoint's stale draft"
    );
    assert!(!resumed["messages"]
        .to_string()
        .contains(SUPPRESSED_TOOL_TEXT));
    assert!(!resumed["messages"].to_string().contains(CORRECTION_HEADER));
    let resumed_trace = host
        .storage
        .get_conversation_turn_trace(ASSISTANT_ID)
        .unwrap()
        .unwrap();
    let visibility_events = resumed_trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::BackendState { content, .. } => {
                serde_json::from_str::<Value>(content)
                    .ok()
                    .filter(|state| state["type"] == "assistant_text_visibility")
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        visibility_events,
        [json!({
            "type": "assistant_text_visibility", "modelRequestIndex": 2,
            "status": "not_shown", "reason": "file_transaction_unsettled",
        })],
        "approval checkpoint must preserve suppression until the complete tool batch resumes"
    );
    assert_call(
        &host,
        &resumed,
        "context-approval-commit",
        "apply_patch",
        &commit,
    );
    assert_eq!(
        tool_result(&host, &resumed, "context-approval-commit")["status"],
        "applied"
    );
    for id in ["context-approval-begin", "context-approval-append"] {
        tool_result(&host, &resumed, id);
    }
    assert_eq!(
        host.storage
            .get_agent_file_change(&transaction_id)
            .unwrap()
            .unwrap()
            .status,
        "applied"
    );
    assert_eq!(
        fs::read_to_string(host.workspace.join("approved.txt")).unwrap(),
        DRAFT_CONTENT
    );
    host.finish().await;
}
