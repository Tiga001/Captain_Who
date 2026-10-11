use super::*;
use mycopilot_protocol_rs::HUMAN_INTERACTION_SUBMIT_METHOD;
use std::collections::VecDeque;

pub(crate) use crate::application::agent::rpc_resources::{RpcResourceScope, RpcSequencingPolicy};
pub(crate) type RpcScopeResolver = Box<dyn FnMut() -> RpcResourceScope + Send>;

pub(crate) fn rpc_requires_queued_access(method: &str) -> bool {
    matches!(
        method,
        AGENT_START_CONVERSATION_TURN_METHOD
            | AGENT_CONTINUE_CONVERSATION_TURN_METHOD
            | AGENT_REWRITE_CONVERSATION_TURN_METHOD
            | HUMAN_INTERACTION_SUBMIT_METHOD
    )
}

/// Both production and performance harness use this exact audit/resolution boundary. A failed
/// lookup cannot grant parallelism; the original handler still reports its normal API error.
pub(crate) fn rpc_scope_resolver(
    service: AgentService,
    request: &JsonRpcRequest,
) -> RpcScopeResolver {
    let method = request.method.clone();
    // Do not clone a multi-megabyte Turn body merely to resolve its identity.
    let params = request.params.as_ref().map(|input| {
        let mut hints = serde_json::Map::new();
        for key in [
            "conversationId",
            "runId",
            "expectedRunId",
            "actionId",
            "requestId",
            "projectId",
            "id",
            "scopeId",
        ] {
            if let Some(value) = input.get(key).and_then(Value::as_str) {
                hints.insert(key.into(), Value::String(value.into()));
            }
        }
        for (outer, inner) in [("turn", "conversationId"), ("draft", "scopeId")] {
            if let Some(value) = input
                .get(outer)
                .and_then(|value| value.get(inner))
                .and_then(Value::as_str)
            {
                hints.insert(
                    outer.into(),
                    Value::Object(serde_json::Map::from_iter([(
                        inner.into(),
                        Value::String(value.into()),
                    )])),
                );
            }
        }
        Value::Object(hints)
    });
    Box::new(move || {
        service
            .rpc_resource_scope(&method, params.as_ref())
            .unwrap_or_else(|error| {
                tracing::warn!(
                    method,
                    error,
                    "RPC scope resolution failed; retaining global sequencing"
                );
                RpcResourceScope::Global
            })
    })
}

impl RpcResourceScope {
    pub(super) fn is_turn_preparation(&self) -> bool {
        matches!(
            self,
            Self::GlobalTurnPreparation { .. }
                | Self::Resources {
                    policy: RpcSequencingPolicy::TurnPreparation { .. },
                    ..
                }
        )
    }

    pub(super) fn changes_ownership(&self) -> bool {
        matches!(
            self,
            Self::Global
                | Self::GlobalTurnPreparation { .. }
                | Self::Resources {
                    policy: RpcSequencingPolicy::OwnershipChange,
                    ..
                }
        )
    }

    pub(super) fn fairness_root(&self) -> String {
        match self {
            Self::Resources { fairness_root, .. } => format!("root:{fairness_root}"),
            Self::Global => "__global__".into(),
            Self::GlobalTurnPreparation { .. } => "__global_preparation__".into(),
            Self::ExecutionAccess => "__execution_access__".into(),
            Self::Overview => "__overview__".into(),
        }
    }

    pub(super) fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Global, _) | (_, Self::Global) => true,
            (Self::GlobalTurnPreparation { .. }, _) | (_, Self::GlobalTurnPreparation { .. }) => {
                true
            }
            (Self::ExecutionAccess, _) | (_, Self::ExecutionAccess) => true,
            (Self::Overview, _) | (_, Self::Overview) => false,
            (
                Self::Resources {
                    keys,
                    fairness_root,
                    ..
                },
                Self::Resources {
                    keys: other,
                    fairness_root: other_root,
                    ..
                },
            ) => {
                keys.iter()
                    .any(|key| !key.starts_with("project-member:") && other.contains(key))
                    || keys
                        .iter()
                        .filter_map(|key| key.strip_prefix("project-marker:"))
                        .any(|project| other.contains(&format!("project-member:{project}")))
                    || other
                        .iter()
                        .filter_map(|key| key.strip_prefix("project-marker:"))
                        .any(|project| keys.contains(&format!("project-member:{project}")))
                    || keys.contains(&format!("tree:{other_root}"))
                    || other.contains(&format!("tree:{fairness_root}"))
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct ResourcePredecessor {
    pub(super) class: RpcDispatchClass,
    pub(super) scope: watch::Receiver<Option<RpcResourceScope>>,
    pub(super) completion: Completion,
    pub(super) started: Arc<AtomicBool>,
    pub(super) preparation: watch::Receiver<Option<bool>>,
}

impl ResourcePredecessor {
    pub(super) fn permits_preparing_cancel(
        &self,
        class: RpcDispatchClass,
        scope: &RpcResourceScope,
        previous: &RpcResourceScope,
    ) -> bool {
        if class != RpcDispatchClass::Control || !self.started.load(Ordering::Acquire) {
            return false;
        }
        let RpcResourceScope::Resources {
            policy: RpcSequencingPolicy::CancelPreparation { conversation_id },
            ..
        } = scope
        else {
            return false;
        };
        let preparing_conversation = match previous {
            RpcResourceScope::Resources {
                policy: RpcSequencingPolicy::TurnPreparation { conversation_id },
                ..
            }
            | RpcResourceScope::GlobalTurnPreparation { conversation_id } => conversation_id,
            _ => return false,
        };
        conversation_id == preparing_conversation
    }

    pub(super) async fn resolved_scope(&mut self) -> RpcResourceScope {
        loop {
            if let Some(previous) = self.scope.borrow_and_update().as_ref() {
                return previous.clone();
            }
            if self.scope.changed().await.is_err() {
                return RpcResourceScope::Global;
            }
        }
    }

    pub(super) async fn wait_completed(&mut self) {
        while !*self.completion.borrow_and_update() {
            if self.completion.changed().await.is_err() {
                break;
            }
        }
    }

    pub(super) async fn conflicts(
        &mut self,
        class: RpcDispatchClass,
        scope: &RpcResourceScope,
    ) -> bool {
        // Concurrent reads do not require ordering. A following mutation does wait for an
        // earlier exact read to release its snapshot, just as a read waits for earlier writes.
        if class == RpcDispatchClass::Read && self.class == RpcDispatchClass::Read {
            return false;
        }
        // Grant publication must also pass queued preparations: they carry their admission-time
        // access token and cannot become newly authorized under the replacement account grant.
        if class == RpcDispatchClass::Control && *scope == RpcResourceScope::ExecutionAccess {
            loop {
                if let Some(preparing) = *self.preparation.borrow_and_update() {
                    if preparing {
                        return false;
                    }
                    break;
                }
                if self.preparation.changed().await.is_err() {
                    break;
                }
            }
        }
        let previous = self.resolved_scope().await;
        // Audited overview reads observe a consistent snapshot, with no resource effects a
        // later mutation must await. In particular a slow system status probe cannot fence
        // cancellation. Keep the reverse direction: an overview still waits for a prior
        // unknown global mutation, and exact reads continue to sequence in both directions.
        if self.class == RpcDispatchClass::Read && previous == RpcResourceScope::Overview {
            return false;
        }
        if self.permits_preparing_cancel(class, scope, &previous) {
            return false;
        }
        scope.overlaps(&previous)
    }
}

/// Bounded ready tasks share physical workers by authoritative root, not arrival bursts. Scope
/// dependencies never consume a physical slot. Every waiter is already charged to its lane's
/// count and byte admission, and no independent unbounded worker queue is introduced.
#[derive(Default)]
struct FairState {
    active: usize,
    active_roots: HashMap<String, usize>,
    queues: HashMap<String, VecDeque<oneshot::Sender<FairPermit>>>,
    roots: VecDeque<String>,
    last_root: Option<String>,
}
pub(super) struct FairExecution {
    limit: usize,
    state: Mutex<FairState>,
}
pub(super) struct FairPermit {
    executor: Arc<FairExecution>,
    root: String,
}

impl FairExecution {
    pub(super) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::new(FairState::default()),
        })
    }

    pub(super) async fn acquire(self: &Arc<Self>, root: String) -> FairPermit {
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.queues.contains_key(&root) {
                state.roots.push_back(root.clone())
            }
            state.queues.entry(root).or_default().push_back(sender);
        }
        self.drive();
        receiver
            .await
            .expect("fair execution owns accepted waiters until drain")
    }

    fn drive(self: &Arc<Self>) {
        let grants = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let mut grants = Vec::new();
            let mut skipped = 0;
            if state.roots.len() > 1 && state.roots.front() == state.last_root.as_ref() {
                let last = state.roots.pop_front().unwrap();
                state.roots.push_back(last);
            }
            while state.active < self.limit
                && !state.roots.is_empty()
                && skipped < state.roots.len()
            {
                let root = state.roots.pop_front().unwrap();
                // One root cannot occupy all physical handler workers. Identity resolution is
                // short metadata work and is bounded by the lane's total physical concurrency.
                if root.starts_with("root:") && state.active_roots.contains_key(&root) {
                    state.roots.push_back(root);
                    skipped += 1;
                    continue;
                }
                skipped = 0;
                let queue = state.queues.get_mut(&root).expect("ready root has a queue");
                let sender = queue.pop_front().expect("ready root is nonempty");
                if queue.is_empty() {
                    state.queues.remove(&root);
                } else {
                    state.roots.push_back(root.clone());
                }
                state.active += 1;
                *state.active_roots.entry(root.clone()).or_default() += 1;
                state.last_root = Some(root.clone());
                grants.push((
                    sender,
                    FairPermit {
                        executor: Arc::clone(self),
                        root,
                    },
                ));
            }
            grants
        };
        // A cancelled receiver drops the grant and returns capacity outside the registry lock.
        for (sender, permit) in grants {
            let _ = sender.send(permit);
        }
    }
}
impl Drop for FairPermit {
    fn drop(&mut self) {
        {
            let mut state = self
                .executor
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            state.active -= 1;
            let active = state
                .active_roots
                .get_mut(&self.root)
                .expect("granted root tracked");
            *active -= 1;
            if *active == 0 {
                state.active_roots.remove(&self.root);
            }
        }
        self.executor.drive();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as sync_mpsc;

    fn scope(conversation: &str, root: &str) -> RpcResourceScope {
        RpcResourceScope::Resources {
            keys: vec![format!("conversation:{conversation}")],
            fairness_root: root.into(),
            policy: RpcSequencingPolicy::Ordered,
        }
    }
    fn submit(
        rpc: &RpcRequestDispatcher,
        class: RpcDispatchClass,
        number: i64,
        scope: RpcResourceScope,
        work: impl FnOnce() + Send + 'static,
    ) {
        rpc.try_submit_scoped(
            class,
            JsonRpcId::Number(number),
            128,
            Box::new(move || scope.clone()),
            move || {
                work();
                response_success(JsonRpcId::Number(number), true)
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn prior_overview_reads_do_not_fence_control_but_global_writes_still_order() {
        let (published, scope_watch) = watch::channel(Some(RpcResourceScope::Overview));
        let (_complete, completion) = watch::channel(false);
        let (_kind, preparation) = watch::channel(Some(false));
        let mut previous = ResourcePredecessor {
            class: RpcDispatchClass::Read,
            scope: scope_watch,
            completion,
            started: Arc::new(AtomicBool::new(true)),
            preparation,
        };
        assert!(
            !previous
                .conflicts(RpcDispatchClass::Control, &RpcResourceScope::Global)
                .await,
            "an ownerless status observation must not delay cancellation"
        );
        published.send_replace(Some(scope("A", "A")));
        assert!(
            previous
                .conflicts(RpcDispatchClass::Control, &RpcResourceScope::Global)
                .await,
            "exact reads still precede an unknown global mutation"
        );
        previous.class = RpcDispatchClass::Write;
        published.send_replace(Some(RpcResourceScope::Global));
        assert!(
            previous
                .conflicts(RpcDispatchClass::Read, &RpcResourceScope::Overview)
                .await,
            "a later overview preserves the prior global mutation's fence"
        );
        assert!(
            previous
                .conflicts(RpcDispatchClass::Write, &scope("A", "A"))
                .await,
            "unknown global writes retain their normal resource ordering"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn independent_writes_start_together_and_exact_reads_keep_both_directions() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        submit(
            &rpc,
            RpcDispatchClass::Write,
            1,
            scope("A", "A"),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
            },
        );
        ready.await.unwrap();
        submit(&rpc, RpcDispatchClass::Read, 2, scope("A", "A"), || {});
        submit(&rpc, RpcDispatchClass::Write, 3, scope("B", "B"), || {});
        let early = tokio::time::timeout(Duration::from_secs(2), responses.recv()).await;
        release.send(()).unwrap();
        assert_eq!(
            early.unwrap().unwrap()["id"],
            3,
            "B must execute while A's preparation and exact read are pending"
        );
        rpc.shutdown().await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert_eq!(responses.recv().await.unwrap()["id"], 2);

        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        submit(
            &rpc,
            RpcDispatchClass::Read,
            4,
            scope("A", "A"),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
            },
        );
        ready.await.unwrap();
        submit(&rpc, RpcDispatchClass::Write, 5, scope("A", "A"), || {});
        submit(&rpc, RpcDispatchClass::Write, 6, scope("B", "B"), || {});
        let early = tokio::time::timeout(Duration::from_secs(2), responses.recv()).await;
        release.send(()).unwrap();
        assert_eq!(early.unwrap().unwrap()["id"], 6);
        rpc.shutdown().await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 4);
        assert_eq!(responses.recv().await.unwrap()["id"], 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn global_barrier_waits_for_all_prior_owners_and_blocks_later_owners() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        submit(
            &rpc,
            RpcDispatchClass::Write,
            1,
            scope("A", "A"),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
            },
        );
        ready.await.unwrap();
        submit(&rpc, RpcDispatchClass::Write, 2, scope("B", "B"), || {});
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            2
        );
        submit(
            &rpc,
            RpcDispatchClass::Write,
            3,
            RpcResourceScope::Global,
            || {},
        );
        submit(&rpc, RpcDispatchClass::Write, 4, scope("C", "C"), || {});
        assert!(
            tokio::time::timeout(Duration::from_millis(60), responses.recv())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        for id in [1, 3, 4] {
            assert_eq!(responses.recv().await.unwrap()["id"], id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_root_cannot_fill_both_workers_or_starve_a_ready_independent_root() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        submit(
            &rpc,
            RpcDispatchClass::Write,
            1,
            scope("child-A", "big-root"),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(4)).unwrap();
            },
        );
        ready.await.unwrap();
        for number in 2..20 {
            submit(
                &rpc,
                RpcDispatchClass::Write,
                number,
                scope(&format!("child-{number}"), "big-root"),
                || {},
            );
        }
        submit(
            &rpc,
            RpcDispatchClass::Write,
            20,
            scope("independent", "independent"),
            || {},
        );
        let early = tokio::time::timeout(Duration::from_secs(2), responses.recv()).await;
        release.send(()).unwrap();
        assert_eq!(
            early.unwrap().unwrap()["id"],
            20,
            "ready independent root owns the spare physical worker"
        );
        rpc.shutdown().await.unwrap();
        let mut completed = 1;
        while responses.recv().await.is_some() {
            completed += 1;
        }
        assert_eq!(completed, 20);
    }

    #[tokio::test]
    async fn cancelled_fair_waiter_returns_its_grant_without_leaking_capacity() {
        let gate = FairExecution::new(1);
        let held = gate.acquire("root:A".into()).await;
        let waiter_gate = Arc::clone(&gate);
        let waiter = tokio::spawn(async move { waiter_gate.acquire("root:B".into()).await });
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        drop(held);
        let next = tokio::time::timeout(Duration::from_secs(1), gate.acquire("root:C".into()))
            .await
            .unwrap();
        drop(next);
        assert_eq!(gate.state.lock().unwrap().active, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn byte_admission_counts_queued_and_active_requests_and_releases_after_failure() {
        let (outbound, _responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        rpc.try_submit_scoped(
            RpcDispatchClass::Write,
            JsonRpcId::Number(1),
            64 * 1024 * 1024,
            Box::new(|| RpcResourceScope::Global),
            move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(3)).unwrap();
                response_success(JsonRpcId::Number(1), true)
            },
        )
        .unwrap();
        ready.await.unwrap();
        let rejected = rpc
            .try_submit_scoped(
                RpcDispatchClass::Write,
                JsonRpcId::Number(2),
                1,
                Box::new(|| RpcResourceScope::Global),
                || unreachable!(),
            )
            .unwrap_err();
        assert_eq!(rejected["error"]["data"]["code"], "overloaded");
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
    }

    #[test]
    fn a_tree_stop_conflicts_with_each_descendant_but_siblings_can_prepare_independently() {
        let first = scope("child-one", "root");
        let second = scope("child-two", "root");
        assert!(!first.overlaps(&second));
        let stop = RpcResourceScope::Resources {
            keys: vec!["tree:root".into()],
            fairness_root: "root".into(),
            policy: RpcSequencingPolicy::Ordered,
        };
        assert!(stop.overlaps(&first));
        assert!(second.overlaps(&stop));
        assert!(!stop.overlaps(&scope("outside", "outside")));
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn project_members_run_in_parallel_but_project_delete_fences_the_complete_membership() {
        let member = |conversation: &str| RpcResourceScope::Resources {
            keys: vec![
                format!("conversation:{conversation}"),
                "project-member:P".into(),
            ],
            fairness_root: conversation.into(),
            policy: RpcSequencingPolicy::Ordered,
        };
        assert!(!member("A").overlaps(&member("B")));
        let project = RpcResourceScope::Resources {
            keys: vec!["project-marker:P".into()],
            fairness_root: "project:P".into(),
            policy: RpcSequencingPolicy::Ordered,
        };
        assert!(project.overlaps(&member("A")));
        assert!(member("B").overlaps(&project));
        assert!(!project.overlaps(&scope("outside", "outside")));
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        submit(&rpc, RpcDispatchClass::Write, 1, member("A"), move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
        });
        ready.await.unwrap();
        submit(&rpc, RpcDispatchClass::Write, 2, member("B"), || {});
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            2
        );
        submit(&rpc, RpcDispatchClass::Write, 3, project, || {});
        submit(&rpc, RpcDispatchClass::Write, 4, member("C"), || {});
        submit(
            &rpc,
            RpcDispatchClass::Write,
            5,
            scope("outside", "outside"),
            || {},
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            5
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        for id in [1, 3, 4] {
            assert_eq!(responses.recv().await.unwrap()["id"], id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_scope_is_refreshed_after_a_move_before_a_later_project_delete() {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let moved = Arc::new(AtomicBool::new(false));
        let (release_move, wait_move) = sync_mpsc::channel();
        let (move_started, move_ready) = oneshot::channel();
        let moved_worker = Arc::clone(&moved);
        submit(
            &rpc,
            RpcDispatchClass::Write,
            1,
            RpcResourceScope::Resources {
                keys: vec![
                    "conversation:A".into(),
                    "project-member:P".into(),
                    "project-member:Q".into(),
                ],
                fairness_root: "A".into(),
                policy: RpcSequencingPolicy::OwnershipChange,
            },
            move || {
                move_started.send(()).unwrap();
                wait_move.recv_timeout(Duration::from_secs(4)).unwrap();
                moved_worker.store(true, Ordering::Release);
            },
        );
        move_ready.await.unwrap();
        let (looked_up, initial_lookup) = oneshot::channel();
        let mut looked_up = Some(looked_up);
        let (read_started, read_ready) = oneshot::channel();
        let (release_read, wait_read) = sync_mpsc::channel();
        rpc.try_submit_scoped(
            RpcDispatchClass::Read,
            JsonRpcId::Number(2),
            128,
            Box::new(move || {
                let project = if moved.load(Ordering::Acquire) {
                    "Q"
                } else {
                    "P"
                };
                if let Some(looked_up) = looked_up.take() {
                    looked_up.send(project).unwrap();
                }
                RpcResourceScope::Resources {
                    keys: vec!["conversation:A".into(), format!("project-member:{project}")],
                    fairness_root: "A".into(),
                    policy: RpcSequencingPolicy::Ordered,
                }
            }),
            move || {
                read_started.send(()).unwrap();
                wait_read.recv_timeout(Duration::from_secs(4)).unwrap();
                response_success(JsonRpcId::Number(2), true)
            },
        )
        .unwrap();
        assert_eq!(initial_lookup.await.unwrap(), "P");
        submit(
            &rpc,
            RpcDispatchClass::Write,
            3,
            RpcResourceScope::Resources {
                keys: vec!["project-marker:Q".into()],
                fairness_root: "project:Q".into(),
                policy: RpcSequencingPolicy::OwnershipChange,
            },
            || {},
        );
        release_move.send(()).unwrap();
        read_ready.await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(60), responses.recv())
                .await
                .is_err(),
            "deletion must see the reader's refreshed destination project membership"
        );
        release_read.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        for id in [2, 3] {
            assert_eq!(responses.recv().await.unwrap()["id"], id);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn verified_preparing_cancel_bypasses_only_its_started_turn_handler() {
        assert_preparing_cancel_bypasses_only_its_started_turn_handler(false).await;
        assert_preparing_cancel_bypasses_only_its_started_turn_handler(true).await;
    }

    async fn assert_preparing_cancel_bypasses_only_its_started_turn_handler(
        is_new_conversation: bool,
    ) {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        let mut preparation = scope("A", "A");
        let RpcResourceScope::Resources { policy, .. } = &mut preparation else {
            unreachable!()
        };
        *policy = RpcSequencingPolicy::TurnPreparation {
            conversation_id: "A".into(),
        };
        if is_new_conversation {
            preparation = RpcResourceScope::GlobalTurnPreparation {
                conversation_id: "A".into(),
            };
        }
        submit(&rpc, RpcDispatchClass::Write, 1, preparation, move || {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(4)).unwrap();
        });
        ready.await.unwrap();
        let mut cancellation = scope("A", "A");
        let RpcResourceScope::Resources { policy, .. } = &mut cancellation else {
            unreachable!()
        };
        *policy = RpcSequencingPolicy::CancelPreparation {
            conversation_id: "A".into(),
        };
        submit(
            &rpc,
            RpcDispatchClass::Control,
            2,
            cancellation.clone(),
            || {},
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            2
        );
        // An intervening ordinary same-resource write preserves its original FIFO boundary.
        submit(&rpc, RpcDispatchClass::Write, 3, scope("A", "A"), || {});
        submit(&rpc, RpcDispatchClass::Control, 4, cancellation, || {});
        assert!(
            tokio::time::timeout(Duration::from_millis(60), responses.recv())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        for id in [1, 3, 4] {
            assert_eq!(responses.recv().await.unwrap()["id"], id);
        }
    }

    #[tokio::test]
    async fn preparing_cancel_does_not_bypass_queued_turns_or_an_unknown_global_fence() {
        let (published, scope_watch) = watch::channel(Some(RpcResourceScope::Resources {
            keys: vec!["conversation:A".into()],
            fairness_root: "A".into(),
            policy: RpcSequencingPolicy::TurnPreparation {
                conversation_id: "A".into(),
            },
        }));
        let (_complete, completion) = watch::channel(false);
        let (_kind, preparation) = watch::channel(Some(false));
        let mut previous = ResourcePredecessor {
            class: RpcDispatchClass::Write,
            scope: scope_watch,
            completion,
            started: Arc::new(AtomicBool::new(false)),
            preparation,
        };
        let cancellation = RpcResourceScope::Resources {
            keys: vec!["conversation:A".into()],
            fairness_root: "A".into(),
            policy: RpcSequencingPolicy::CancelPreparation {
                conversation_id: "A".into(),
            },
        };
        assert!(
            previous
                .conflicts(RpcDispatchClass::Control, &cancellation)
                .await
        );
        previous.started.store(true, Ordering::Release);
        assert!(
            !previous
                .conflicts(RpcDispatchClass::Control, &cancellation)
                .await
        );
        published.send_replace(Some(RpcResourceScope::GlobalTurnPreparation {
            conversation_id: "B".into(),
        }));
        assert!(
            previous
                .conflicts(RpcDispatchClass::Control, &cancellation)
                .await,
            "a new Conversation's global preparation requires the exact Core owner"
        );
        published.send_replace(Some(RpcResourceScope::GlobalTurnPreparation {
            conversation_id: "A".into(),
        }));
        assert!(
            !previous
                .conflicts(RpcDispatchClass::Control, &cancellation)
                .await
        );
        published.send_replace(Some(RpcResourceScope::Global));
        assert!(
            previous
                .conflicts(RpcDispatchClass::Control, &cancellation)
                .await
        );
        assert!(
            previous
                .conflicts(
                    RpcDispatchClass::Control,
                    &RpcResourceScope::ExecutionAccess
                )
                .await
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn revoke_bypasses_running_and_queued_preparations_without_reauthorizing_their_tokens() {
        assert_revoke_bypasses_preparations(false).await;
        assert_revoke_bypasses_preparations(true).await;
    }

    async fn assert_revoke_bypasses_preparations(first_has_unknown_owner: bool) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("queued-access.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let service = AgentService::new_authorized_for_test(storage);
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let queued_access = service.capture_execution_access();
        let preparation = RpcResourceScope::Resources {
            keys: vec!["conversation:A".into()],
            fairness_root: "A".into(),
            policy: RpcSequencingPolicy::TurnPreparation {
                conversation_id: "A".into(),
            },
        };
        let (release, wait) = sync_mpsc::channel();
        let (started, ready) = oneshot::channel();
        let preparing_service = service.clone();
        submit(
            &rpc,
            RpcDispatchClass::Write,
            1,
            if first_has_unknown_owner {
                RpcResourceScope::GlobalTurnPreparation {
                    conversation_id: "A".into(),
                }
            } else {
                preparation.clone()
            },
            move || {
                preparing_service.with_queued_execution_access(queued_access, || {
                    let token = preparing_service.capture_execution_access().unwrap();
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(4)).unwrap();
                    assert!(preparing_service
                        .check_execution_access_token(token)
                        .is_err());
                });
            },
        );
        ready.await.unwrap();
        let queued_service = service.clone();
        let queued_access = service.capture_execution_access();
        submit(&rpc, RpcDispatchClass::Write, 2, preparation, move || {
            queued_service.with_queued_execution_access(queued_access, || {
                assert!(
                    queued_service.capture_execution_access().is_err(),
                    "a relogin cannot authorize a request already retained in the queue"
                );
            });
        });
        let revoke_service = service.clone();
        submit(
            &rpc,
            RpcDispatchClass::Control,
            3,
            RpcResourceScope::ExecutionAccess,
            move || {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64;
                revoke_service
                    .set_execution_access(mycopilot_protocol_rs::SetExecutionAccessInput {
                        revision: 2,
                        identity_epoch: 0,
                        reason: mycopilot_protocol_rs::ExecutionAccessReason::AccountSignedOut,
                        issued_at: now,
                        valid_until: now,
                    })
                    .unwrap();
            },
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            3
        );
        submit(
            &rpc,
            RpcDispatchClass::Control,
            4,
            RpcResourceScope::ExecutionAccess,
            move || {
                service.grant_execution_access_for_test();
            },
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), responses.recv())
                .await
                .unwrap()
                .unwrap()["id"],
            4
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        for id in [1, 2] {
            let response = responses.recv().await.unwrap();
            assert_eq!(response["id"], id);
            assert!(response.get("error").is_none(), "{response}");
        }
    }
}
