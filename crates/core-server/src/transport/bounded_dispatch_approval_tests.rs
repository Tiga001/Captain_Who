use super::*;
use std::sync::mpsc as sync_mpsc;

fn request(id: i64, method: &str, params: Value) -> JsonRpcRequest {
    serde_json::from_value(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
        .unwrap()
}

fn submit(
    rpc: &RpcRequestDispatcher,
    request: JsonRpcRequest,
    alias: Option<RpcOwnerResolver>,
    task: impl FnOnce() -> Value + Send + 'static,
) {
    rpc.try_submit_ordered(
        rpc_dispatch_class(&request),
        request.id.clone(),
        rpc_ordering_key(&request),
        alias,
        rpc_has_global_mutation_fence(&request),
        task,
    )
    .unwrap();
}

fn alias(conversation: &'static str) -> Option<RpcOwnerResolver> {
    Some(Box::new(move || {
        Some(format!("conversation:{conversation}"))
    }))
}

fn ok(id: i64) -> Value {
    response_success(JsonRpcId::Number(id), true)
}

#[test]
fn approval_priority_uses_an_explicit_scoped_write_allowlist() {
    for method in [AGENT_APPROVE_ACTION_METHOD, AGENT_REJECT_ACTION_METHOD] {
        assert_eq!(
            rpc_dispatch_class(&request(1, method, json!({"runId":"r","actionId":"a"}))),
            RpcDispatchClass::Approval
        );
    }
    for method in [
        STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
        STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD,
        STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
    ] {
        assert!(!rpc_has_global_mutation_fence(&request(
            1,
            method,
            json!({"conversationId":"c"})
        )));
        assert!(rpc_has_global_mutation_fence(&request(
            1,
            method,
            json!({})
        )));
        // An unrelated top-level run hint must not turn a storage mutation into a bypassable
        // write under the wrong owner, even if DTO validation later rejects that extra field.
        assert!(rpc_has_global_mutation_fence(&request(
            1,
            method,
            json!({"conversationId":"c", "runId":"other-run"})
        )));
    }
    for method in [
        "unknown.mutation",
        mycopilot_protocol_rs::CORE_SET_EXECUTION_ACCESS_METHOD,
        STORAGE_DELETE_CONVERSATION_METHOD,
        AGENT_START_CONVERSATION_TURN_METHOD,
        AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
    ] {
        assert!(
            rpc_has_global_mutation_fence(&request(
                1,
                method,
                json!({"conversationId":"c","rootConversationId":"root"})
            )),
            "{method}"
        );
    }
    assert_eq!(
        rpc_dispatch_class(&request(
            1,
            AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
            json!({"rootConversationId":"root","approvalId":"a"})
        )),
        RpcDispatchClass::Write
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_approval_owner_falls_back_to_the_admission_time_write_fence() {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let (started, ready) = oneshot::channel();
    let (release, wait) = sync_mpsc::channel();
    let committed = Arc::new(AtomicBool::new(false));
    let save_committed = Arc::clone(&committed);
    submit(
        &rpc,
        request(
            1,
            STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
            json!({"conversationId":"A"}),
        ),
        None,
        move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            save_committed.store(true, Ordering::Release);
            ok(1)
        },
    );
    ready.await.unwrap();
    submit(
        &rpc,
        request(
            2,
            AGENT_APPROVE_ACTION_METHOD,
            json!({"runId":"not-yet-resolved","actionId":"a"}),
        ),
        Some(Box::new(|| None)),
        move || {
            assert!(committed.load(Ordering::Acquire));
            ok(2)
        },
    );
    release.send(()).unwrap();
    rpc.shutdown().await.unwrap();
    assert_eq!(responses.recv().await.unwrap()["id"], 1);
    assert_eq!(responses.recv().await.unwrap()["id"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_admission_is_bounded_independent_of_cancel_and_shutdown_drains_it() {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let (started, ready) = oneshot::channel();
    let (release, wait) = sync_mpsc::channel();
    submit(
        &rpc,
        request(1, "global.mutation", json!({})),
        None,
        move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            ok(1)
        },
    );
    ready.await.unwrap();
    for index in 0..APPROVAL_CAPACITY {
        submit(
            &rpc,
            request(
                index as i64 + 2,
                AGENT_APPROVE_ACTION_METHOD,
                json!({"runId":"run-A","actionId":index.to_string()}),
            ),
            alias("A"),
            || panic!("approval waits for the global fence and must not run after shutdown"),
        );
    }
    let overflow = request(
        100,
        AGENT_APPROVE_ACTION_METHOD,
        json!({"runId":"run-A","actionId":"overflow"}),
    );
    let response = rpc
        .try_submit_ordered(
            rpc_dispatch_class(&overflow),
            overflow.id.clone(),
            rpc_ordering_key(&overflow),
            alias("A"),
            false,
            || unreachable!(),
        )
        .unwrap_err();
    assert_eq!(response["error"]["data"]["code"], "overloaded");
    submit(
        &rpc,
        request(101, AGENT_CANCEL_RUN_METHOD, json!({"runId":"run-B"})),
        alias("B"),
        || ok(101),
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), responses.recv())
            .await
            .unwrap()
            .unwrap()["id"],
        101
    );
    rpc.begin_shutdown();
    release.send(()).unwrap();
    rpc.shutdown().await.unwrap();
    assert_eq!(responses.recv().await.unwrap()["id"], 1);
    let mut ids = Vec::new();
    while let Some(response) = responses.recv().await {
        assert_eq!(response["error"]["data"]["code"], "shutting_down");
        ids.push(response["id"].as_i64().unwrap());
    }
    ids.sort();
    assert_eq!(ids, (2..APPROVAL_CAPACITY as i64 + 2).collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_bypasses_unrelated_scoped_writes_but_waits_for_its_own_snapshot() {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let (started, ready) = oneshot::channel();
    let (release, wait) = sync_mpsc::channel();
    submit(
        &rpc,
        request(
            1,
            STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
            json!({"conversationId":"A"}),
        ),
        None,
        move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            ok(1)
        },
    );
    ready.await.unwrap();
    submit(
        &rpc,
        request(
            2,
            AGENT_APPROVE_ACTION_METHOD,
            json!({"runId":"run-A","actionId":"a"}),
        ),
        alias("A"),
        || ok(2),
    );
    submit(
        &rpc,
        request(
            3,
            AGENT_REJECT_ACTION_METHOD,
            json!({"runId":"run-B","actionId":"b"}),
        ),
        alias("B"),
        || ok(3),
    );
    let first = tokio::time::timeout(Duration::from_secs(2), responses.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first["id"], 3,
        "another conversation's rejection must not wait behind A's storage backlog"
    );
    release.send(()).unwrap();
    rpc.shutdown().await.unwrap();
    assert_eq!(responses.recv().await.unwrap()["id"], 1);
    assert_eq!(responses.recv().await.unwrap()["id"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_respects_global_mutations_even_when_the_dto_contains_an_owner() {
    for (method, params) in [
        ("unknown.mutation", json!({"conversationId":"unrelated"})),
        (
            STORAGE_DELETE_CONVERSATION_METHOD,
            json!({"conversationId":"A"}),
        ),
        (
            AGENT_START_CONVERSATION_TURN_METHOD,
            json!({"conversationId":"A"}),
        ),
        (
            AGENT_REWRITE_CONVERSATION_TURN_METHOD,
            json!({"turn":{"conversationId":"A"}}),
        ),
        (
            AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD,
            json!({"rootConversationId":"root","approvalId":"child"}),
        ),
    ] {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        let committed = Arc::new(AtomicBool::new(false));
        let write_committed = Arc::clone(&committed);
        submit(&rpc, request(1, method, params), None, move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            write_committed.store(true, Ordering::Release);
            ok(1)
        });
        ready.await.unwrap();
        submit(
            &rpc,
            request(
                2,
                AGENT_APPROVE_ACTION_METHOD,
                json!({"runId":"run-B","actionId":"a"}),
            ),
            alias("B"),
            move || {
                assert!(
                    committed.load(Ordering::Acquire),
                    "approval overtook global mutation {method}"
                );
                ok(2)
            },
        );
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert_eq!(responses.recv().await.unwrap()["id"], 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn priority_decisions_preserve_run_and_conversation_alias_order_in_both_directions() {
    for approval_first in [false, true] {
        let (outbound, mut responses) = crate::transport::outbound_channel();
        let rpc = RpcRequestDispatcher::new(outbound);
        let (started, ready) = oneshot::channel();
        let (release, wait) = sync_mpsc::channel();
        let committed = Arc::new(AtomicBool::new(false));
        let prior_committed = Arc::clone(&committed);
        let approval = |id| {
            request(
                id,
                AGENT_APPROVE_ACTION_METHOD,
                json!({"runId":"run-A","actionId":"a"}),
            )
        };
        let cancel = |id| {
            request(
                id,
                mycopilot_protocol_rs::AGENT_CANCEL_MANUAL_CONTEXT_COMPACTION_METHOD,
                json!({"conversationId":"A","operationId":"compact"}),
            )
        };
        submit(
            &rpc,
            if approval_first {
                approval(1)
            } else {
                cancel(1)
            },
            if approval_first { alias("A") } else { None },
            move || {
                started.send(()).unwrap();
                wait.recv().unwrap();
                prior_committed.store(true, Ordering::Release);
                ok(1)
            },
        );
        ready.await.unwrap();
        submit(
            &rpc,
            if approval_first {
                cancel(2)
            } else {
                approval(2)
            },
            if approval_first { None } else { alias("A") },
            move || {
                assert!(
                    committed.load(Ordering::Acquire),
                    "same owner must retain approval/cancel order"
                );
                ok(2)
            },
        );
        submit(
            &rpc,
            request(3, AGENT_CANCEL_RUN_METHOD, json!({"runId":"run-B"})),
            alias("B"),
            || ok(3),
        );
        let first = tokio::time::timeout(Duration::from_secs(2), responses.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first["id"], 3, "an unrelated cancel stays responsive");
        release.send(()).unwrap();
        rpc.shutdown().await.unwrap();
        assert_eq!(responses.recv().await.unwrap()["id"], 1);
        assert_eq!(responses.recv().await.unwrap()["id"], 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_run_ids_in_one_conversation_share_the_priority_fence() {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let (started, ready) = oneshot::channel();
    let (release, wait) = sync_mpsc::channel();
    let committed = Arc::new(AtomicBool::new(false));
    let prior_committed = Arc::clone(&committed);
    submit(
        &rpc,
        request(
            1,
            AGENT_APPROVE_ACTION_METHOD,
            json!({"runId":"old-run","actionId":"a"}),
        ),
        alias("A"),
        move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            prior_committed.store(true, Ordering::Release);
            ok(1)
        },
    );
    ready.await.unwrap();
    submit(
        &rpc,
        request(2, AGENT_CANCEL_RUN_METHOD, json!({"runId":"new-run"})),
        alias("A"),
        move || {
            assert!(committed.load(Ordering::Acquire));
            ok(2)
        },
    );
    submit(
        &rpc,
        request(
            3,
            AGENT_APPROVE_ACTION_METHOD,
            json!({"runId":"run-B","actionId":"b"}),
        ),
        alias("B"),
        || ok(3),
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), responses.recv())
            .await
            .unwrap()
            .unwrap()["id"],
        3
    );
    release.send(()).unwrap();
    rpc.shutdown().await.unwrap();
    assert_eq!(responses.recv().await.unwrap()["id"], 1);
    assert_eq!(responses.recv().await.unwrap()["id"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn later_writes_and_reads_wait_for_the_approval_acknowledgement() {
    let (outbound, mut responses) = crate::transport::outbound_channel();
    let rpc = RpcRequestDispatcher::new(outbound);
    let (started, ready) = oneshot::channel();
    let (release, wait) = sync_mpsc::channel();
    let committed = Arc::new(AtomicBool::new(false));
    let decision_committed = Arc::clone(&committed);
    submit(
        &rpc,
        request(
            1,
            AGENT_APPROVE_ACTION_METHOD,
            json!({"runId":"run-A","actionId":"a"}),
        ),
        alias("A"),
        move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
            decision_committed.store(true, Ordering::Release);
            ok(1)
        },
    );
    ready.await.unwrap();
    for (id, method) in [
        (2, STORAGE_DELETE_CONVERSATION_METHOD),
        (3, STORAGE_LOAD_CONVERSATION_METHOD),
    ] {
        let committed = Arc::clone(&committed);
        submit(
            &rpc,
            request(id, method, json!({"conversationId":"A"})),
            None,
            move || {
                assert!(committed.load(Ordering::Acquire));
                ok(id)
            },
        );
    }
    release.send(()).unwrap();
    rpc.shutdown().await.unwrap();
    for id in 1..=3 {
        assert_eq!(responses.recv().await.unwrap()["id"], id);
    }
}
