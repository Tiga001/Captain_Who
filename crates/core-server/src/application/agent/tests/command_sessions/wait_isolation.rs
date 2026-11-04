use super::*;

struct CrossDomainAgentWaitStore {
    polls: AtomicUsize,
    replies: StdMutex<VecDeque<Option<AgentWaitReadySnapshot>>>,
}

impl CrossDomainAgentWaitStore {
    fn pending() -> Self {
        Self {
            polls: AtomicUsize::new(0),
            replies: StdMutex::new(VecDeque::from([None, None])),
        }
    }

    fn publish(&self, ready: AgentWaitReadySnapshot) {
        self.replies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push_back(Some(ready));
    }
}

impl crate::application::agent_wait::AgentWaitStore for CrossDomainAgentWaitStore {
    fn poll_ready(
        &self,
        _input: &mycopilot_core::PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, mycopilot_core::AgentGraphError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .replies
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
            .flatten())
    }
}

fn cross_domain_ready() -> AgentWaitReadySnapshot {
    AgentWaitReadySnapshot {
        receipt: AgentModelBatchReceiptRecord {
            receipt_id: "agent-wait-receipt".into(),
            agent_id: "caller".into(),
            conversation_id: "agent-conversation".into(),
            run_id: "agent-run".into(),
            assistant_message_id: "agent-assistant".into(),
            model_batch_index: 2,
            sampling_bound_at: Some(1),
            created_at: 1,
            updated_at: 1,
        },
        source_receipt_id: None,
        targets: vec![AgentWaitTargetSnapshot {
            target_agent_id: "target".into(),
            target_task_name: "review".into(),
            messages: Vec::new(),
            target_status_version: 2,
            latest_wake_sequence: Some(1),
            latest_wake_status_revision: Some(2),
            latest_wake_status: Some(mycopilot_core::AgentWakeStatus::Completed),
            display_status: AgentDisplayStatus::LatestCompleted,
        }],
        model_projection: AgentWaitModelProjection::PrecommittedToolResult,
    }
}

fn persist_wait_only_command(
    fixture: &RunningFixture,
    session_id: &str,
    call_id: &str,
    started_at: u64,
    with_output: bool,
) {
    fixture
        .storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.into(),
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: call_id.into(),
                project_id: None,
                command: "deterministic fake command".into(),
                cwd: fixture.workspace.path().to_string_lossy().into_owned(),
                command_digest: ZERO_DIGEST.into(),
                status: AgentCommandSessionStatus::Starting,
                started_at,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({"decision":"approved"}),
            permission_provenance: json!({"mode":"test"}),
            created_at: i64::try_from(started_at).unwrap(),
        })
        .unwrap();
    fixture
        .storage
        .mark_agent_command_session_running(
            &fixture.conversation_id,
            session_id,
            i64::try_from(started_at + 1).unwrap(),
        )
        .unwrap();
    if with_output {
        fixture
            .storage
            .append_agent_command_session_output(&AgentCommandSessionOutputAppend {
                conversation_id: &fixture.conversation_id,
                session_id,
                chunks: &[AgentCommandSessionOutputChunk {
                    sequence: 1,
                    stream: AgentCommandOutputStream::Stdout,
                    output: "durable fake output".into(),
                }],
                latest_sequence: 1,
                transcript_truncated: false,
                output_capture_truncated: false,
                updated_at: i64::try_from(started_at + 2).unwrap(),
            })
            .unwrap();
    }
    fixture.registry.install_wait_only_test_session(
        session_id,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        call_id,
    );
}

fn settle_wait_only_command(
    fixture: &RunningFixture,
    session_id: &str,
    latest_sequence: u64,
    committed_at: i64,
) {
    fixture
        .storage
        .settle_agent_command_session(&AgentCommandSessionTerminalUpdate {
            conversation_id: &fixture.conversation_id,
            session_id,
            status: AgentCommandSessionStatus::Exited,
            ended_at: u64::try_from(committed_at).unwrap(),
            exit_code: Some(0),
            latest_sequence,
            transcript_truncated: false,
            output_capture_truncated: false,
            archive_ref: None,
            terminal_reason: None,
            published_outputs: &[],
            artifact_observation: None,
            committed_at,
        })
        .unwrap();
    fixture.registry.signal_wait_only_test_terminal(session_id);
}

#[tokio::test]
async fn real_registry_command_wait_and_agent_wait_are_isolated_without_shell() {
    use crate::application::agent_wait::{
        AgentWaitKernel, AgentWaitNotifications, AgentWaitOutcome,
    };

    let fixture = Arc::new(RunningFixture::new("dual-wait-registry"));
    let notifications = AgentWaitNotifications::default();
    const FIRST: &str = "cmd_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    persist_wait_only_command(&fixture, FIRST, "call-command-first", 100, true);
    let command_wait = {
        let fixture = Arc::clone(&fixture);
        tokio::task::spawn_blocking(move || {
            fixture
                .registry
                .wait_for_live_terminal(FIRST, Duration::from_secs(2))
        })
    };
    let store = Arc::new(CrossDomainAgentWaitStore::pending());
    let kernel = AgentWaitKernel::test_with_store(store.clone(), notifications.clone());
    let agent_wait = tokio::spawn(async move {
        kernel
            .wait(
                mycopilot_core::PollAgentWaitInput {
                    caller_agent_id: "caller".into(),
                    conversation_id: "agent-conversation".into(),
                    run_id: "agent-run".into(),
                    assistant_message_id: "agent-assistant".into(),
                    model_batch_index: 2,
                    target_agent_ids: vec!["target".into()],
                    maximum_messages: 1,
                },
                Duration::from_secs(2),
                mycopilot_core::AgentCancellationToken::new(),
                mycopilot_core::AgentCancellationToken::new(),
                None,
            )
            .await
            .unwrap()
    });
    while store.polls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    settle_wait_only_command(&fixture, FIRST, 1, 104);
    assert!(command_wait.await.unwrap());
    assert!(
        !agent_wait.is_finished(),
        "command terminal must not wake Agent wait"
    );
    let read = fixture
        .storage
        .read_or_create_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: FIRST,
            run_id: "command-read-run",
            call_id: "command-read-call",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: 1_024,
            host_output_truncated: false,
            created_at: 105,
        })
        .unwrap()
        .unwrap();
    assert_eq!(read.receipt.status, AgentCommandSessionStatus::Exited);
    assert_eq!(read.chunks[0].output, "durable fake output");
    store.publish(cross_domain_ready());
    notifications.notify_caller("caller");
    assert!(matches!(
        agent_wait.await.unwrap(),
        AgentWaitOutcome::Ready(_)
    ));

    const SECOND: &str = "cmd_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    persist_wait_only_command(&fixture, SECOND, "call-command-second", 110, false);
    let second_command_wait = {
        let fixture = Arc::clone(&fixture);
        tokio::task::spawn_blocking(move || {
            fixture
                .registry
                .wait_for_live_terminal(SECOND, Duration::from_secs(2))
        })
    };
    let second_store = Arc::new(CrossDomainAgentWaitStore::pending());
    let second_kernel =
        AgentWaitKernel::test_with_store(second_store.clone(), notifications.clone());
    let second_agent_wait = tokio::spawn(async move {
        second_kernel
            .wait(
                mycopilot_core::PollAgentWaitInput {
                    caller_agent_id: "caller".into(),
                    conversation_id: "agent-conversation".into(),
                    run_id: "agent-run-second".into(),
                    assistant_message_id: "agent-assistant-second".into(),
                    model_batch_index: 2,
                    target_agent_ids: vec!["target".into()],
                    maximum_messages: 1,
                },
                Duration::from_secs(2),
                mycopilot_core::AgentCancellationToken::new(),
                mycopilot_core::AgentCancellationToken::new(),
                None,
            )
            .await
            .unwrap()
    });
    while second_store.polls.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    second_store.publish(cross_domain_ready());
    notifications.notify_caller("caller");
    assert!(matches!(
        second_agent_wait.await.unwrap(),
        AgentWaitOutcome::Ready(_)
    ));
    assert!(
        !second_command_wait.is_finished(),
        "Agent result must not wake the production Registry command waiter"
    );
    settle_wait_only_command(&fixture, SECOND, 0, 114);
    assert!(second_command_wait.await.unwrap());
    let second_read = fixture
        .storage
        .read_or_create_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: SECOND,
            run_id: "command-read-run-second",
            call_id: "command-read-call-second",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: 1_024,
            host_output_truncated: false,
            created_at: 115,
        })
        .unwrap()
        .unwrap();
    assert_eq!(
        second_read.receipt.status,
        AgentCommandSessionStatus::Exited
    );
    assert!(second_read.chunks.is_empty());
}
