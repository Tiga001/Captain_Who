//! Caller-owned wait kernel for persistent Agent collaboration.
//!
//! This is deliberately not a model Tool. Round 4 may adapt it, but the reliable check/register/
//! check algorithm and authorization/cursor semantics live here rather than in transport code.

use mycopilot_core::storage::service::StorageService;
use mycopilot_core::{
    AgentCancellationToken, AgentGraphError, AgentSteerInputQueue, AgentWaitReadySnapshot,
    PollAgentWaitInput,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;

pub(crate) trait AgentWaitStore: Send + Sync {
    fn poll_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError>;
}

impl AgentWaitStore for StorageService {
    fn poll_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError> {
        self.poll_agent_wait_ready(input)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentWaitStopReason {
    TimedOut,
    Cancelled,
    Shutdown,
    InterruptedBySteer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentWaitOutcome {
    Ready(AgentWaitReadySnapshot),
    Stopped(AgentWaitStopReason),
}

#[derive(Clone, Default)]
pub(crate) struct AgentWaitNotifications {
    callers: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
}

impl AgentWaitNotifications {
    fn listener(&self, caller_agent_id: &str) -> Arc<Notify> {
        let mut callers = self
            .callers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        callers
            .entry(caller_agent_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    /// Call only after the Mailbox/Wake transaction commits. False positives are harmless because
    /// every waiter re-reads SQLite; a signal never carries the result itself.
    pub(crate) fn notify_caller(&self, caller_agent_id: &str) {
        self.listener(caller_agent_id).notify_waiters();
    }
}

/// One process-wide latency accelerator shared by messaging, Dispatcher and wait callers. SQLite
/// remains authoritative, so a second Host or a lost notification is covered by bounded polling.
pub(crate) fn shared_agent_wait_notifications() -> AgentWaitNotifications {
    static SHARED: OnceLock<AgentWaitNotifications> = OnceLock::new();
    SHARED.get_or_init(AgentWaitNotifications::default).clone()
}

#[derive(Clone)]
pub(crate) struct AgentWaitKernel {
    storage: Arc<dyn AgentWaitStore>,
    notifications: AgentWaitNotifications,
}

impl AgentWaitKernel {
    pub(crate) fn new(storage: Arc<StorageService>, notifications: AgentWaitNotifications) -> Self {
        Self {
            storage,
            notifications,
        }
    }

    pub(crate) fn production(storage: Arc<StorageService>) -> Self {
        Self::new(storage, shared_agent_wait_notifications())
    }

    #[cfg(test)]
    pub(crate) fn test_with_store(
        storage: Arc<dyn AgentWaitStore>,
        notifications: AgentWaitNotifications,
    ) -> Self {
        Self {
            storage,
            notifications,
        }
    }

    pub(crate) async fn wait(
        &self,
        input: PollAgentWaitInput,
        timeout: Duration,
        cancellation: AgentCancellationToken,
        shutdown: AgentCancellationToken,
        steer: Option<AgentSteerInputQueue>,
    ) -> Result<AgentWaitOutcome, AgentGraphError> {
        if let Some(reason) = wait_stop_reason(&cancellation, &shutdown, steer.as_ref()) {
            return Ok(AgentWaitOutcome::Stopped(reason));
        }
        // First check: avoid registration work when a durable item is already ready.
        if let Some(ready) = self.storage.poll_ready(&input)? {
            return Ok(AgentWaitOutcome::Ready(ready));
        }
        let listener = self.notifications.listener(&input.caller_agent_id);
        let notified = listener.notified();
        tokio::pin!(notified);
        if let Some(reason) = wait_stop_reason(&cancellation, &shutdown, steer.as_ref()) {
            return Ok(AgentWaitOutcome::Stopped(reason));
        }
        // Second check closes the check/subscription lost-wakeup window.
        if let Some(ready) = self.storage.poll_ready(&input)? {
            return Ok(AgentWaitOutcome::Ready(ready));
        }
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        // Cross-process commits cannot publish into this process-local accelerator. A short
        // bounded durable recheck is therefore part of correctness, while Notify removes the
        // usual latency for same-process commits.
        let mut durable_recheck = tokio::time::interval(Duration::from_millis(50));
        durable_recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        durable_recheck.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = async {
                    if let Some(steer) = steer.as_ref() {
                        steer.changed().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    return Ok(AgentWaitOutcome::Stopped(AgentWaitStopReason::InterruptedBySteer));
                }
                _ = cancellation.cancelled() => {
                    return Ok(AgentWaitOutcome::Stopped(AgentWaitStopReason::Cancelled));
                }
                _ = shutdown.cancelled() => {
                    return Ok(AgentWaitOutcome::Stopped(AgentWaitStopReason::Shutdown));
                }
                _ = &mut deadline => {
                    return Ok(AgentWaitOutcome::Stopped(AgentWaitStopReason::TimedOut));
                }
                _ = &mut notified => {
                    if let Some(reason) = wait_stop_reason(&cancellation, &shutdown, steer.as_ref()) {
                        return Ok(AgentWaitOutcome::Stopped(reason));
                    }
                    if let Some(ready) = self.storage.poll_ready(&input)? {
                        return Ok(AgentWaitOutcome::Ready(ready));
                    }
                    notified.set(listener.notified());
                }
                _ = durable_recheck.tick() => {
                    if let Some(reason) = wait_stop_reason(&cancellation, &shutdown, steer.as_ref()) {
                        return Ok(AgentWaitOutcome::Stopped(reason));
                    }
                    if let Some(ready) = self.storage.poll_ready(&input)? {
                        return Ok(AgentWaitOutcome::Ready(ready));
                    }
                }
            }
        }
    }
}

fn wait_stop_reason(
    cancellation: &AgentCancellationToken,
    shutdown: &AgentCancellationToken,
    steer: Option<&AgentSteerInputQueue>,
) -> Option<AgentWaitStopReason> {
    if steer.is_some_and(|queue| queue.pending_len() > 0) {
        Some(AgentWaitStopReason::InterruptedBySteer)
    } else if cancellation.is_cancelled() {
        Some(AgentWaitStopReason::Cancelled)
    } else if shutdown.is_cancelled() {
        Some(AgentWaitStopReason::Shutdown)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::{
        AgentDisplayStatus, AgentModelBatchReceiptRecord, AgentWaitTargetSnapshot,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeStore {
        polls: AtomicUsize,
        replies: Mutex<VecDeque<Option<AgentWaitReadySnapshot>>>,
    }

    impl FakeStore {
        fn new(replies: impl IntoIterator<Item = Option<AgentWaitReadySnapshot>>) -> Self {
            Self {
                polls: AtomicUsize::new(0),
                replies: Mutex::new(replies.into_iter().collect()),
            }
        }

        fn publish(&self, ready: AgentWaitReadySnapshot) {
            self.replies
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push_back(Some(ready));
        }
    }

    impl AgentWaitStore for FakeStore {
        fn poll_ready(
            &self,
            _input: &PollAgentWaitInput,
        ) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(self
                .replies
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
                .flatten())
        }
    }

    fn input() -> PollAgentWaitInput {
        PollAgentWaitInput {
            caller_agent_id: "caller".into(),
            conversation_id: "conversation".into(),
            run_id: "run".into(),
            assistant_message_id: "assistant".into(),
            model_batch_index: 2,
            target_agent_ids: vec!["target-a".into(), "target-b".into()],
            maximum_messages: 32,
        }
    }

    fn ready() -> AgentWaitReadySnapshot {
        AgentWaitReadySnapshot {
            receipt: AgentModelBatchReceiptRecord {
                receipt_id: "receipt".into(),
                agent_id: "caller".into(),
                conversation_id: "conversation".into(),
                run_id: "run".into(),
                assistant_message_id: "assistant".into(),
                model_batch_index: 2,
                sampling_bound_at: None,
                created_at: 1,
                updated_at: 1,
            },
            targets: vec![AgentWaitTargetSnapshot {
                target_agent_id: "target-b".into(),
                messages: Vec::new(),
                target_status_version: 4,
                latest_wake_sequence: Some(2),
                latest_wake_status_revision: Some(4),
                latest_wake_status: Some(mycopilot_core::AgentWakeStatus::Completed),
                display_status: AgentDisplayStatus::LatestCompleted,
            }],
            model_projection: mycopilot_core::AgentWaitModelProjection::PrecommittedToolResult,
        }
    }

    fn controls() -> (
        AgentCancellationToken,
        AgentCancellationToken,
        AgentSteerInputQueue,
    ) {
        (
            AgentCancellationToken::new(),
            AgentCancellationToken::new(),
            AgentSteerInputQueue::new(),
        )
    }

    #[tokio::test]
    async fn returns_existing_pending_without_waiting() {
        let store = Arc::new(FakeStore::new([Some(ready())]));
        let kernel = AgentWaitKernel {
            storage: store.clone(),
            notifications: AgentWaitNotifications::default(),
        };
        let (cancel, shutdown, steer) = controls();
        assert!(matches!(
            kernel
                .wait(
                    input(),
                    Duration::from_secs(60),
                    cancel,
                    shutdown,
                    Some(steer)
                )
                .await
                .unwrap(),
            AgentWaitOutcome::Ready(_)
        ));
        assert_eq!(store.polls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn notification_after_registration_rechecks_durable_store() {
        let store = Arc::new(FakeStore::new([None, None]));
        let notifications = AgentWaitNotifications::default();
        let kernel = AgentWaitKernel {
            storage: store.clone(),
            notifications: notifications.clone(),
        };
        let (cancel, shutdown, steer) = controls();
        let waiter = tokio::spawn(async move {
            kernel
                .wait(
                    input(),
                    Duration::from_secs(2),
                    cancel,
                    shutdown,
                    Some(steer),
                )
                .await
                .unwrap()
        });
        while store.polls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        store.publish(ready());
        notifications.notify_caller("caller");
        assert!(matches!(waiter.await.unwrap(), AgentWaitOutcome::Ready(_)));
    }

    #[tokio::test]
    async fn durable_fallback_observes_another_process_without_notification() {
        let store = Arc::new(FakeStore::new([None, None]));
        let kernel = AgentWaitKernel {
            storage: store.clone(),
            notifications: AgentWaitNotifications::default(),
        };
        let (cancel, shutdown, steer) = controls();
        let waiter = tokio::spawn(async move {
            kernel
                .wait(
                    input(),
                    Duration::from_secs(2),
                    cancel,
                    shutdown,
                    Some(steer),
                )
                .await
                .unwrap()
        });
        while store.polls.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        store.publish(ready());
        assert!(matches!(waiter.await.unwrap(), AgentWaitOutcome::Ready(_)));
        assert!(store.polls.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn timeout_cancel_shutdown_and_steer_are_independent_stop_domains() {
        let run = |cancelled: bool, shutdown_requested: bool, steer_requested: bool| async move {
            let store = Arc::new(FakeStore::new([None, None]));
            let kernel = AgentWaitKernel {
                storage: store,
                notifications: AgentWaitNotifications::default(),
            };
            let (cancel, shutdown, steer) = controls();
            if cancelled {
                cancel.cancel();
            }
            if shutdown_requested {
                shutdown.cancel();
            }
            if steer_requested {
                steer
                    .enqueue(mycopilot_core::AgentSteerInput {
                        guidance_id: "guidance".into(),
                        client_message_id: "client".into(),
                        content: "steer".into(),
                        attachments: Vec::new(),
                        attachment_library: None,
                        created_at: 1,
                    })
                    .unwrap();
            }
            kernel
                .wait(
                    input(),
                    Duration::from_millis(5),
                    cancel,
                    shutdown,
                    Some(steer),
                )
                .await
                .unwrap()
        };
        assert_eq!(
            run(false, false, false).await,
            AgentWaitOutcome::Stopped(AgentWaitStopReason::TimedOut)
        );
        assert_eq!(
            run(true, false, false).await,
            AgentWaitOutcome::Stopped(AgentWaitStopReason::Cancelled)
        );
        assert_eq!(
            run(false, true, false).await,
            AgentWaitOutcome::Stopped(AgentWaitStopReason::Shutdown)
        );
        assert_eq!(
            run(false, false, true).await,
            AgentWaitOutcome::Stopped(AgentWaitStopReason::InterruptedBySteer)
        );
    }

    #[tokio::test]
    async fn steer_wins_over_simultaneously_ready_result_without_polling_it() {
        let store = Arc::new(FakeStore::new([Some(ready())]));
        let kernel = AgentWaitKernel {
            storage: store.clone(),
            notifications: AgentWaitNotifications::default(),
        };
        let (cancel, shutdown, steer) = controls();
        steer
            .enqueue(mycopilot_core::AgentSteerInput {
                guidance_id: "steer-wins".into(),
                client_message_id: "client-steer-wins".into(),
                content: "user steer".into(),
                attachments: Vec::new(),
                attachment_library: None,
                created_at: 1,
            })
            .unwrap();
        assert_eq!(
            kernel
                .wait(
                    input(),
                    Duration::from_secs(1),
                    cancel,
                    shutdown,
                    Some(steer),
                )
                .await
                .unwrap(),
            AgentWaitOutcome::Stopped(AgentWaitStopReason::InterruptedBySteer)
        );
        assert_eq!(store.polls.load(Ordering::SeqCst), 0);
        let (cancel, shutdown, _) = controls();
        assert!(matches!(
            kernel
                .wait(input(), Duration::from_secs(1), cancel, shutdown, None)
                .await
                .unwrap(),
            AgentWaitOutcome::Ready(_)
        ));
    }
}
