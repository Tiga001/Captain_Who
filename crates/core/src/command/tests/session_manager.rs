use super::*;
use crate::command::session::CommandSessionCompletionHook;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};

fn test_config() -> CommandSessionManagerConfig {
    CommandSessionManagerConfig {
        max_active_sessions: 8,
        max_active_sessions_per_scope: 4,
        max_retained_terminal_sessions: 32,
        transcript_bytes: 4 * 1024,
        poll_bytes: 4 * 1024,
        drain_grace: Duration::from_millis(300),
        interrupt_grace: Duration::from_millis(100),
        shutdown_grace: Duration::from_secs(2),
    }
}

fn start_unchecked(
    manager: &CommandSessionManager,
    workspace: &TestWorkspace,
    scope: &str,
    command: &str,
    yield_for: Duration,
    hard_timeout: Option<Duration>,
) -> CommandStartOutcome {
    manager
        .start_plan(
            CommandSessionScopeId::new(scope).unwrap(),
            CommandSpawnPlan::shell(
                command.to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                hard_timeout,
            ),
            CommandStartOptions {
                initial_yield: yield_for,
            },
            None,
            None,
        )
        .unwrap()
}

fn running_id(outcome: CommandStartOutcome) -> CommandSessionId {
    match outcome {
        CommandStartOutcome::Running(snapshot) => snapshot.session_id,
        CommandStartOutcome::Exited(result) => {
            panic!("expected running session, got {:?}", result.snapshot.state)
        }
    }
}

fn wait_terminal(
    manager: &CommandSessionManager,
    session_id: &CommandSessionId,
) -> CommandTerminalResult {
    manager
        .wait_terminal_result(session_id, Duration::from_secs(3))
        .unwrap()
        .expect("session should reach a terminal state")
}

#[test]
fn short_command_exits_inside_initial_yield() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "short",
        "printf hello",
        Duration::from_millis(500),
        None,
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("short command should return a terminal result");
    };
    assert_eq!(
        terminal.snapshot.state,
        CommandSessionState::Exited { exit_code: Some(0) }
    );
    assert_eq!(terminal.execution.stdout, "hello");
}

#[test]
fn long_command_returns_running_without_an_implicit_hard_timeout() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "long",
        "sleep 1",
        Duration::from_millis(10),
        None,
    ));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        manager.snapshot(&id).unwrap().state,
        CommandSessionState::Running
    );
    manager
        .force_terminate(&id, Duration::from_secs(2))
        .unwrap();
}

#[test]
fn model_poll_is_incremental_and_timeline_reads_are_non_destructive() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "poll",
        "printf one; sleep 0.15; printf two; sleep 0.15",
        Duration::from_millis(10),
        None,
    ));

    let timeline = manager.read_output(&id, 0, Duration::from_secs(1)).unwrap();
    let first_poll = manager.poll(&id, Duration::from_secs(1)).unwrap();
    assert_eq!(
        timeline.output.chunks, first_poll.output.chunks,
        "timeline reading must not drain the model cursor"
    );
    let first_text = first_poll
        .output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert!(first_text.contains("one"));

    let second_poll = manager.poll(&id, Duration::from_secs(1)).unwrap();
    let second_text = second_poll
        .output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert!(!second_text.contains("one"));
    assert!(second_text.contains("two"));
    wait_terminal(&manager, &id);

    let replay = manager.read_output(&id, 0, Duration::ZERO).unwrap();
    let replay_text = replay
        .output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>();
    assert!(replay_text.contains("one"));
    assert!(replay_text.contains("two"));
}

#[test]
fn terminal_or_deadline_read_aggregates_noisy_output_until_the_deadline() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "aggregate-noisy-output",
        "i=0; while [ \"$i\" -lt 40 ]; do printf 'tick-%02d\\n' \"$i\"; i=$((i + 1)); sleep 0.02; done; sleep 1",
        Duration::from_millis(10),
        None,
    ));

    let wait = Duration::from_millis(220);
    let started = Instant::now();
    let poll = manager
        .read_output_until_terminal_or_deadline(&id, 0, wait)
        .unwrap();
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(180),
        "ordinary output must not end the aggregate wait early: {elapsed:?}"
    );
    assert_eq!(poll.snapshot.state, CommandSessionState::Running);
    assert!(poll.output.latest_sequence > 1);
    assert!(poll
        .output
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>()
        .contains("tick-"));

    manager
        .force_terminate(&id, Duration::from_secs(2))
        .unwrap();
}

#[test]
fn terminal_or_deadline_read_returns_as_soon_as_terminal_is_published() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "aggregate-terminal",
        "sleep 0.1; printf done",
        Duration::from_millis(10),
        None,
    ));

    let started = Instant::now();
    let poll = manager
        .read_output_until_terminal_or_deadline(&id, 0, Duration::from_secs(5))
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "terminal publication must end the aggregate wait"
    );
    assert_eq!(
        poll.snapshot.state,
        CommandSessionState::Exited { exit_code: Some(0) }
    );
    assert_eq!(
        poll.output
            .chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<String>(),
        "done"
    );
}

#[test]
fn terminal_or_deadline_reads_are_bounded_and_page_without_loss() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.transcript_bytes = 8 * 1024;
    config.poll_bytes = MIN_COMMAND_POLL_BYTES;
    let manager = CommandSessionManager::new(config).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "aggregate-page",
        "awk 'BEGIN { for (i = 0; i < 3000; i++) printf \"x\" }'",
        Duration::from_millis(500),
        None,
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("command should finish inside yield");
    };

    let mut after = 0;
    let mut total = 0;
    loop {
        let batch = manager
            .read_output_until_terminal_or_deadline(
                &terminal.snapshot.session_id,
                after,
                Duration::ZERO,
            )
            .unwrap()
            .output;
        let bytes = batch
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
        assert!(bytes <= MIN_COMMAND_POLL_BYTES);
        total += bytes;
        let Some(last) = batch.chunks.last() else {
            break;
        };
        after = last.sequence;
    }
    assert_eq!(total, 3000);
}

#[test]
fn concurrent_model_polls_on_one_session_are_serialized() {
    let workspace = TestWorkspace::new();
    let manager = Arc::new(CommandSessionManager::new(test_config()).unwrap());
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "serial-poll",
        "sleep 0.1; printf unique; sleep 0.15",
        Duration::from_millis(10),
        None,
    ));
    let barrier = Arc::new(Barrier::new(3));
    let readers = (0..2)
        .map(|_| {
            let manager = manager.clone();
            let id = id.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                manager.poll(&id, Duration::from_secs(1)).unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let occurrences = readers
        .into_iter()
        .map(|reader| reader.join().unwrap())
        .flat_map(|poll| poll.output.chunks)
        .filter(|chunk| chunk.text.contains("unique"))
        .count();
    assert_eq!(occurrences, 1);
    wait_terminal(&manager, &id);
}

#[test]
fn output_has_one_monotonic_sequence_and_preserves_utf8() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "ordering",
        "printf '你'; sleep 0.02; printf err >&2; sleep 0.02; printf '好'",
        Duration::from_millis(500),
        None,
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("command should finish inside yield");
    };
    let output = manager
        .read_output(&terminal.snapshot.session_id, 0, Duration::ZERO)
        .unwrap()
        .output;
    assert!(output
        .chunks
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    assert!(output.chunks.iter().any(|chunk| {
        chunk.stream == AgentCommandOutputStream::Stdout && chunk.text.contains('你')
    }));
    assert!(output.chunks.iter().any(|chunk| {
        chunk.stream == AgentCommandOutputStream::Stderr && chunk.text.contains("err")
    }));
    assert!(output
        .chunks
        .iter()
        .all(|chunk| std::str::from_utf8(chunk.text.as_bytes()).is_ok()));
}

#[test]
fn lifecycle_events_are_serialized_from_started_through_terminal() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.transcript_bytes = 256 * 1024;
    let manager = CommandSessionManager::new(config).unwrap();
    let (sender, receiver) = mpsc::channel();
    let callbacks_in_flight = Arc::new(AtomicUsize::new(0));
    let callbacks_overlapped = Arc::new(AtomicBool::new(false));
    let observer: CommandSessionLifecycleObserver = {
        let callbacks_in_flight = Arc::clone(&callbacks_in_flight);
        let callbacks_overlapped = Arc::clone(&callbacks_overlapped);
        Arc::new(move |event| {
            if callbacks_in_flight.fetch_add(1, Ordering::SeqCst) != 0 {
                callbacks_overlapped.store(true, Ordering::SeqCst);
            }
            // Widen the race window: stdout and stderr are drained by independent threads.
            thread::sleep(Duration::from_millis(1));
            let _ = sender.send(event);
            callbacks_in_flight.fetch_sub(1, Ordering::SeqCst);
        })
    };
    let outcome = manager
        .start_plan_with_observers(
            CommandSessionScopeId::new("lifecycle-order").unwrap(),
            CommandSpawnPlan::shell(
                "(awk 'BEGIN { for (i = 0; i < 200; i++) print \"stdout\" }') & \
                 (awk 'BEGIN { for (i = 0; i < 200; i++) print \"stderr\" }' >&2) & wait"
                    .to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                None,
            ),
            CommandStartOptions {
                initial_yield: Duration::from_millis(10),
            },
            None,
            Some(observer),
            None,
            None,
        )
        .unwrap();
    let session_id = match outcome {
        CommandStartOutcome::Running(snapshot) => snapshot.session_id,
        CommandStartOutcome::Exited(terminal) => terminal.snapshot.session_id,
    };

    let mut events = Vec::new();
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("lifecycle observer must deliver a terminal event");
        let terminal = matches!(event, CommandSessionLifecycleEvent::Terminal(_));
        events.push(event);
        if terminal {
            break;
        }
    }
    wait_terminal(&manager, &session_id);

    assert!(matches!(
        events.first(),
        Some(CommandSessionLifecycleEvent::Started(snapshot))
            if snapshot.session_id == session_id
                && snapshot.state == CommandSessionState::Running
                && snapshot.latest_output_sequence == 0
    ));
    assert!(matches!(
        events.last(),
        Some(CommandSessionLifecycleEvent::Terminal(terminal))
            if terminal.snapshot.session_id == session_id
                && terminal.snapshot.state == CommandSessionState::Exited { exit_code: Some(0) }
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CommandSessionLifecycleEvent::Started(_)))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CommandSessionLifecycleEvent::Terminal(_)))
            .count(),
        1
    );

    let output = events
        .iter()
        .filter_map(|event| match event {
            CommandSessionLifecycleEvent::Output {
                session_id: output_session_id,
                chunk,
                latest_sequence,
                ..
            } => Some((output_session_id, chunk, latest_sequence)),
            CommandSessionLifecycleEvent::Started(_)
            | CommandSessionLifecycleEvent::Terminal(_) => None,
        })
        .collect::<Vec<_>>();
    assert!(!output.is_empty());
    assert!(output.iter().all(|(output_session_id, chunk, latest)| {
        *output_session_id == &session_id && **latest == chunk.sequence
    }));
    assert!(output
        .windows(2)
        .all(|pair| pair[0].1.sequence < pair[1].1.sequence));
    assert!(output
        .iter()
        .any(|(_, chunk, _)| chunk.stream == AgentCommandOutputStream::Stdout));
    assert!(output
        .iter()
        .any(|(_, chunk, _)| chunk.stream == AgentCommandOutputStream::Stderr));

    let final_sequence = output.last().unwrap().1.sequence;
    let CommandSessionLifecycleEvent::Terminal(terminal) = events.last().unwrap() else {
        unreachable!("terminal event asserted above")
    };
    assert_eq!(terminal.snapshot.latest_output_sequence, final_sequence);
    assert!(terminal.execution.stdout.contains("stdout"));
    assert!(terminal.execution.stderr.contains("stderr"));
    assert!(
        !callbacks_overlapped.load(Ordering::SeqCst),
        "lifecycle callbacks must never overlap across stdout, stderr, and watcher threads"
    );
}

#[test]
fn terminal_wait_linearizes_after_terminal_lifecycle_delivery() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let terminal_release = Arc::new(Barrier::new(2));
    let lifecycle_delivered = Arc::new(AtomicBool::new(false));
    let (terminal_entered_tx, terminal_entered_rx) = mpsc::channel();
    let observer: CommandSessionLifecycleObserver = {
        let terminal_release = Arc::clone(&terminal_release);
        let lifecycle_delivered = Arc::clone(&lifecycle_delivered);
        Arc::new(move |event| {
            if matches!(event, CommandSessionLifecycleEvent::Terminal(_)) {
                terminal_entered_tx.send(()).unwrap();
                terminal_release.wait();
                lifecycle_delivered.store(true, Ordering::Release);
            }
        })
    };
    let outcome = manager
        .start_plan_with_observers(
            CommandSessionScopeId::new("terminal-publication").unwrap(),
            CommandSpawnPlan::shell(
                "while [ ! -f terminal-release ]; do sleep 0.01; done".to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                None,
            ),
            CommandStartOptions {
                initial_yield: Duration::from_millis(10),
            },
            None,
            Some(observer),
            None,
            None,
        )
        .unwrap();
    let session_id = running_id(outcome);
    let waiter_manager = manager.clone();
    let (waiter_started_tx, waiter_started_rx) = mpsc::channel();
    let (waiter_done_tx, waiter_done_rx) = mpsc::channel();
    let waiter = thread::spawn(move || {
        waiter_started_tx.send(()).unwrap();
        let terminal = waiter_manager
            .wait_terminal_result(&session_id, Duration::from_secs(3))
            .unwrap()
            .expect("terminal result should be published");
        waiter_done_tx.send(terminal).unwrap();
    });

    waiter_started_rx.recv().unwrap();
    std::fs::write(workspace.path.join("terminal-release"), b"release").unwrap();
    terminal_entered_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("terminal lifecycle callback should start");
    assert!(matches!(
        waiter_done_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    terminal_release.wait();
    let terminal = waiter_done_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("terminal wait should complete after lifecycle delivery");
    assert_eq!(
        terminal.snapshot.state,
        CommandSessionState::Exited { exit_code: Some(0) }
    );
    assert!(lifecycle_delivered.load(Ordering::Acquire));
    waiter.join().unwrap();
}

#[test]
fn initial_yield_does_not_publish_terminal_before_lifecycle_delivery() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let terminal_release = Arc::new(Barrier::new(2));
    let (terminal_entered_tx, terminal_entered_rx) = mpsc::channel();
    let observer: CommandSessionLifecycleObserver = {
        let terminal_release = Arc::clone(&terminal_release);
        Arc::new(move |event| {
            if matches!(event, CommandSessionLifecycleEvent::Terminal(_)) {
                terminal_entered_tx.send(()).unwrap();
                terminal_release.wait();
            }
        })
    };
    let start_manager = manager.clone();
    let workspace_path = workspace.path.clone();
    let starter = thread::spawn(move || {
        start_manager
            .start_plan_with_observers(
                CommandSessionScopeId::new("start-publication").unwrap(),
                CommandSpawnPlan::shell(
                    "printf done".to_string(),
                    workspace_path.clone(),
                    Some(&workspace_path),
                    None,
                ),
                CommandStartOptions {
                    initial_yield: Duration::from_millis(50),
                },
                None,
                Some(observer),
                None,
                None,
            )
            .unwrap()
    });

    terminal_entered_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("terminal lifecycle callback should start");
    let outcome = starter
        .join()
        .expect("finite initial yield must not wait for a blocked observer");
    let CommandStartOutcome::Running(snapshot) = outcome else {
        panic!("unpublished terminal must not escape through the start outcome");
    };
    assert_eq!(snapshot.state, CommandSessionState::Running);
    let session_id = snapshot.session_id;
    assert_eq!(
        manager.snapshot(&session_id).unwrap().state,
        CommandSessionState::Running
    );
    assert_eq!(
        manager
            .list(None)
            .into_iter()
            .find(|candidate| candidate.session_id == session_id)
            .expect("session should remain listed")
            .state,
        CommandSessionState::Running
    );
    assert_eq!(
        manager
            .poll(&session_id, Duration::ZERO)
            .unwrap()
            .snapshot
            .state,
        CommandSessionState::Running
    );
    assert_eq!(
        manager
            .read_output(&session_id, 0, Duration::ZERO)
            .unwrap()
            .snapshot
            .state,
        CommandSessionState::Running
    );
    assert_eq!(
        manager
            .interrupt(&session_id, Duration::ZERO)
            .expect("control must not target an internally terminal process")
            .state,
        CommandSessionState::Running
    );
    terminal_release.wait();
    let terminal = wait_terminal(&manager, &session_id);
    assert_eq!(
        terminal.snapshot.state,
        CommandSessionState::Exited { exit_code: Some(0) }
    );
}

#[test]
fn managed_runtime_integrity_completion_error_marks_session_failed() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let (lifecycle_tx, lifecycle_rx) = mpsc::channel();
    let lifecycle_observer: CommandSessionLifecycleObserver =
        Arc::new(move |event| lifecycle_tx.send(event).unwrap());
    let completion_hook: CommandSessionCompletionHook = Box::new(|result| {
        result.error = Some("managed runtime integrity verification failed".to_string());
    });
    let outcome = manager
        .start_plan_with_observers(
            CommandSessionScopeId::new("managed-integrity-failure").unwrap(),
            CommandSpawnPlan::shell(
                "printf completed".to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                None,
            ),
            CommandStartOptions {
                initial_yield: Duration::from_secs(1),
            },
            None,
            Some(lifecycle_observer),
            None,
            Some(completion_hook),
        )
        .unwrap();
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("short command should settle inside initial yield");
    };
    assert_eq!(terminal.snapshot.state, CommandSessionState::Failed);
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(
        terminal.snapshot.error.as_deref(),
        Some("managed runtime integrity verification failed")
    );
    assert_eq!(terminal.execution.exit_code, Some(0));
    assert_eq!(terminal.execution.stdout, "completed");
    assert_eq!(terminal.execution.error, terminal.snapshot.error);

    let lifecycle_terminal = lifecycle_rx
        .into_iter()
        .find_map(|event| match event {
            CommandSessionLifecycleEvent::Terminal(terminal) => Some(terminal),
            CommandSessionLifecycleEvent::Started(_)
            | CommandSessionLifecycleEvent::Output { .. } => None,
        })
        .expect("terminal lifecycle event should be delivered");
    assert_eq!(
        lifecycle_terminal.snapshot.state,
        CommandSessionState::Failed
    );
    assert_eq!(lifecycle_terminal.snapshot.error, terminal.snapshot.error);
}

#[test]
fn completion_hook_panic_marks_successful_process_failed() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let completion_hook: CommandSessionCompletionHook =
        Box::new(|_| panic!("simulated completion hook panic"));
    let outcome = manager
        .start_plan_with_observers(
            CommandSessionScopeId::new("completion-hook-panic").unwrap(),
            CommandSpawnPlan::shell(
                "exit 0".to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                None,
            ),
            CommandStartOptions {
                initial_yield: Duration::from_secs(1),
            },
            None,
            None,
            None,
            Some(completion_hook),
        )
        .unwrap();
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("short command should settle inside initial yield");
    };
    assert_eq!(terminal.snapshot.state, CommandSessionState::Failed);
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(
        terminal.snapshot.error.as_deref(),
        Some("命令终态结算回调异常退出。")
    );
    assert_eq!(terminal.execution.exit_code, Some(0));
    assert_eq!(terminal.execution.error, terminal.snapshot.error);
}

#[test]
fn bounded_transcript_marks_eviction_and_keeps_head_and_tail() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.transcript_bytes = 24;
    config.poll_bytes = 1024;
    let manager = CommandSessionManager::new(config).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "bounded",
        "printf 'HEAD-abcdefghijklmnopqrstuvwxyz-TAIL'",
        Duration::from_millis(500),
        None,
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("command should finish inside yield");
    };
    assert!(terminal.snapshot.output_truncated);
    let batch = manager
        .read_output(&terminal.snapshot.session_id, 0, Duration::ZERO)
        .unwrap()
        .output;
    assert!(batch.truncated_before);
    let text = batch
        .chunks
        .into_iter()
        .map(|chunk| chunk.text)
        .collect::<String>();
    assert!(text.starts_with("HEAD-"));
    assert!(text.ends_with("TAIL"));
}

#[test]
fn each_poll_respects_the_configured_output_byte_limit() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.transcript_bytes = 8 * 1024;
    config.poll_bytes = MIN_COMMAND_POLL_BYTES;
    let manager = CommandSessionManager::new(config).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "poll-limit",
        "awk 'BEGIN { for (i = 0; i < 3000; i++) printf \"x\" }'",
        Duration::from_millis(500),
        None,
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("command should finish inside yield");
    };
    let mut after = 0;
    let mut total = 0;
    loop {
        let batch = manager
            .read_output(&terminal.snapshot.session_id, after, Duration::ZERO)
            .unwrap()
            .output;
        let bytes = batch
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>();
        assert!(bytes <= MIN_COMMAND_POLL_BYTES);
        total += bytes;
        let Some(last) = batch.chunks.last() else {
            break;
        };
        after = last.sequence;
    }
    assert_eq!(total, 3000);
}

#[test]
fn hard_timeout_can_fire_after_the_running_handoff() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "late-timeout",
        "sleep 30",
        Duration::from_millis(10),
        Some(Duration::from_millis(100)),
    ));
    let terminal = wait_terminal(&manager, &id);
    assert_eq!(terminal.snapshot.state, CommandSessionState::TimedOut);
    assert!(terminal.execution.timed_out);
}

#[test]
fn hard_timeout_is_independent_from_initial_yield() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "timeout",
        "sleep 30",
        Duration::from_secs(1),
        Some(Duration::from_millis(50)),
    );
    let CommandStartOutcome::Exited(terminal) = outcome else {
        panic!("hard timeout should finish before the yield window");
    };
    assert_eq!(terminal.snapshot.state, CommandSessionState::TimedOut);
    assert!(terminal.execution.timed_out);
}

#[test]
fn interrupt_terminates_the_process_group() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "interrupt",
        "sleep 30 & wait",
        Duration::from_millis(10),
        None,
    ));
    let snapshot = manager.interrupt(&id, Duration::from_secs(2)).unwrap();
    assert_eq!(snapshot.state, CommandSessionState::Interrupted);
    assert!(wait_terminal(&manager, &id).execution.cancelled);
}

#[test]
fn force_terminate_cleans_up_a_process_ignoring_sigint() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.interrupt_grace = Duration::from_secs(5);
    let manager = CommandSessionManager::new(config).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "force",
        "trap '' INT; sleep 30 & wait",
        Duration::from_millis(10),
        None,
    ));
    let first = manager.interrupt(&id, Duration::from_millis(30)).unwrap();
    assert_eq!(first.state, CommandSessionState::Running);
    let forced = manager
        .force_terminate(&id, Duration::from_secs(2))
        .unwrap();
    assert_eq!(forced.state, CommandSessionState::Interrupted);
}

#[test]
fn different_sessions_run_concurrently_and_limits_are_enforced() {
    let workspace = TestWorkspace::new();
    let mut config = test_config();
    config.max_active_sessions = 2;
    config.max_active_sessions_per_scope = 1;
    let manager = CommandSessionManager::new(config).unwrap();
    let first = running_id(start_unchecked(
        &manager,
        &workspace,
        "one",
        "sleep 30",
        Duration::from_millis(10),
        None,
    ));
    let same_scope = manager.start_plan(
        CommandSessionScopeId::new("one").unwrap(),
        CommandSpawnPlan::shell(
            "sleep 30".to_string(),
            workspace.path.clone(),
            Some(&workspace.path),
            None,
        ),
        CommandStartOptions {
            initial_yield: Duration::from_millis(10),
        },
        None,
        None,
    );
    assert_eq!(
        same_scope.unwrap_err(),
        CommandSessionError::ScopeLimitReached
    );
    let second = running_id(start_unchecked(
        &manager,
        &workspace,
        "two",
        "sleep 30",
        Duration::from_millis(10),
        None,
    ));
    let global = manager.start_plan(
        CommandSessionScopeId::new("three").unwrap(),
        CommandSpawnPlan::shell(
            "sleep 30".to_string(),
            workspace.path.clone(),
            Some(&workspace.path),
            None,
        ),
        CommandStartOptions::default(),
        None,
        None,
    );
    assert_eq!(global.unwrap_err(), CommandSessionError::GlobalLimitReached);
    assert_eq!(
        manager
            .list(None)
            .iter()
            .filter(|item| item.state == CommandSessionState::Running)
            .count(),
        2
    );
    manager
        .force_terminate(&first, Duration::from_secs(2))
        .unwrap();
    manager
        .force_terminate(&second, Duration::from_secs(2))
        .unwrap();
}

#[test]
fn leader_exit_does_not_wait_forever_for_descendant_pipe_holders() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let started = Instant::now();
    let outcome = start_unchecked(
        &manager,
        &workspace,
        "pipe",
        "sleep 30 &",
        Duration::from_secs(2),
        None,
    );
    assert!(matches!(outcome, CommandStartOutcome::Exited(_)));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn terminate_all_and_last_manager_drop_leave_no_leader_process() {
    let workspace = TestWorkspace::new();
    let pid_file = workspace.path.join("managed.pid");
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "drop",
        "echo $$ > managed.pid; trap '' INT; sleep 30 & wait",
        Duration::from_millis(30),
        None,
    ));
    for _ in 0..50 {
        if pid_file.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let clone = manager.clone();
    drop(clone);
    assert_eq!(
        manager.snapshot(&id).unwrap().state,
        CommandSessionState::Running
    );
    drop(manager);
    // SAFETY: signal 0 performs existence/permission probing only.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "last manager drop must reap the command leader");
}

#[test]
fn terminate_all_is_parallel_and_shutdown_closes_the_start_gate() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let first = running_id(start_unchecked(
        &manager,
        &workspace,
        "shutdown-one",
        "sleep 30",
        Duration::from_millis(10),
        None,
    ));
    let second = running_id(start_unchecked(
        &manager,
        &workspace,
        "shutdown-two",
        "sleep 30",
        Duration::from_millis(10),
        None,
    ));
    let report = manager.terminate_all(Duration::from_secs(2));
    assert_eq!(report.requested, 2);
    assert_eq!(report.terminated, 2);
    assert!(report.still_running.is_empty());
    assert!(manager.snapshot(&first).unwrap().state.is_terminal());
    assert!(manager.snapshot(&second).unwrap().state.is_terminal());

    manager.shutdown(Duration::from_secs(1));
    let error = manager
        .start_plan(
            CommandSessionScopeId::new("closed").unwrap(),
            CommandSpawnPlan::shell(
                "printf should-not-run".to_string(),
                workspace.path.clone(),
                Some(&workspace.path),
                None,
            ),
            CommandStartOptions::default(),
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error, CommandSessionError::ManagerShuttingDown);
}

#[test]
fn shutdown_does_not_wait_behind_a_long_model_poll() {
    let workspace = TestWorkspace::new();
    let manager = Arc::new(CommandSessionManager::new(test_config()).unwrap());
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "shutdown-poll",
        "sleep 30",
        Duration::from_millis(10),
        None,
    ));
    let polling_manager = manager.clone();
    let polling_id = id.clone();
    let poller = thread::spawn(move || {
        polling_manager
            .poll(&polling_id, Duration::from_secs(30))
            .unwrap()
    });
    thread::sleep(Duration::from_millis(30));
    let started = Instant::now();
    let report = manager.shutdown(Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(report.terminated, 1);
    assert!(poller.join().unwrap().snapshot.state.is_terminal());
}

#[test]
fn authorized_start_keeps_background_escape_policy_fail_closed() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let permissions = AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        ..Default::default()
    };
    for command in ["sleep 30 &", "nohup sleep 30", "setsid sleep 30"] {
        let error = manager
            .start_authorized_command(
                CommandSessionScopeId::new("security").unwrap(),
                Some(&workspace.path),
                &request(command, None),
                permissions,
                CommandAuthorizationSource::ExplicitUser,
                CommandStartOptions {
                    initial_yield: Duration::from_millis(10),
                },
                None,
            )
            .unwrap_err();
        assert!(matches!(error, CommandSessionStartError::Authorization(_)));
    }
    assert!(manager.list(None).is_empty());
}

#[test]
fn interrupt_kills_descendants_before_they_can_write_a_marker() {
    let workspace = TestWorkspace::new();
    let manager = CommandSessionManager::new(test_config()).unwrap();
    let marker = workspace.path.join("descendant-marker");
    let id = running_id(start_unchecked(
        &manager,
        &workspace,
        "descendant",
        "sh -c 'sleep 0.3; printf escaped > descendant-marker' & wait",
        Duration::from_millis(10),
        None,
    ));
    manager.interrupt(&id, Duration::from_secs(2)).unwrap();
    thread::sleep(Duration::from_millis(450));
    assert!(!marker.exists());
}

#[test]
fn session_ids_round_trip_and_reject_malformed_values() {
    let id = CommandSessionId::new();
    assert_eq!(CommandSessionId::parse(id.as_str()).unwrap(), id);
    for invalid in [
        "",
        "cmd_123",
        "run_0123456789abcdef0123456789abcdef",
        "cmd_0123456789abcdef0123456789abcdeg",
    ] {
        assert_eq!(
            CommandSessionId::parse(invalid).unwrap_err(),
            CommandSessionError::InvalidSessionId
        );
    }
}
