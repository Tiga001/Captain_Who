use super::*;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(5);

fn wait_queued(admission: &KeyedAdmission, count: usize) {
    let deadline = Instant::now() + DEADLINE;
    let mut state = admission.state();
    while state.queue.len() < count {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("admission waiter did not register");
        let (next, timed_out) = admission.changed.wait_timeout(state, remaining).unwrap();
        state = next;
        assert!(
            !timed_out.timed_out() || state.queue.len() >= count,
            "admission waiter did not register"
        );
    }
}

fn assert_drained(admission: &KeyedAdmission) {
    let state = admission.state();
    assert!(!state.global);
    assert!(state.active_keys.is_empty());
    assert!(state.owners.is_empty());
    assert!(state.queue.is_empty());
}

#[test]
fn keyed_admission_unrelated_conversation_passes_blocked_same_key_waiter() {
    let admission = Arc::new(KeyedAdmission::default());
    let first = admission.lock_conversation("a");
    let (same_tx, same_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let same = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("a");
        same_tx.send(()).unwrap();
    });
    wait_queued(&admission, 1);
    let (other_tx, other_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let other = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("b");
        other_tx.send(()).unwrap();
    });
    other_rx.recv_timeout(DEADLINE).unwrap();
    assert!(same_rx.try_recv().is_err());
    other.join().unwrap();
    drop(first);
    same_rx.recv_timeout(DEADLINE).unwrap();
    same.join().unwrap();
    assert_drained(&admission);
}

#[test]
fn keyed_admission_waiting_global_cannot_be_overtaken_by_new_conversation() {
    let admission = Arc::new(KeyedAdmission::default());
    let first = admission.lock_conversation("a");
    let (global_tx, global_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let global = std::thread::spawn(move || {
        let _guard = worker.lock().unwrap_or_else(|error| error.into_inner());
        global_tx.send(()).unwrap();
        release_rx.recv_timeout(DEADLINE).unwrap();
    });
    wait_queued(&admission, 1);
    let (later_tx, later_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let later = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("b");
        later_tx.send(()).unwrap();
    });
    wait_queued(&admission, 2);
    assert!(global_rx.try_recv().is_err());
    assert!(later_rx.try_recv().is_err());
    drop(first);
    global_rx.recv_timeout(DEADLINE).unwrap();
    assert!(later_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    later_rx.recv_timeout(DEADLINE).unwrap();
    global.join().unwrap();
    later.join().unwrap();
    assert_drained(&admission);
}

#[test]
fn keyed_admission_conflicting_requests_are_fifo() {
    let admission = Arc::new(KeyedAdmission::default());
    let initial = admission.lock_conversation("conversation");
    let (entered_tx, entered_rx) = mpsc::channel();
    let mut workers = Vec::new();
    for index in 0..8 {
        let worker_admission = Arc::clone(&admission);
        let entered = entered_tx.clone();
        workers.push(std::thread::spawn(move || {
            let _guard = worker_admission.lock_conversation("conversation");
            entered.send(index).unwrap();
        }));
        wait_queued(&admission, index + 1);
    }
    assert!(entered_rx.try_recv().is_err());
    drop(initial);
    for expected in 0..8 {
        assert_eq!(entered_rx.recv_timeout(DEADLINE).unwrap(), expected);
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_drained(&admission);
}

#[test]
fn keyed_admission_multikey_claim_is_atomic_sorted_and_fair() {
    let admission = Arc::new(KeyedAdmission::default());
    let initial = admission.lock_conversation("b");
    let (multi_tx, multi_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let multi = std::thread::spawn(move || {
        let _guard = worker.lock_conversations(&["b".into(), "a".into(), "b".into()]);
        multi_tx.send(()).unwrap();
        release_rx.recv_timeout(DEADLINE).unwrap();
    });
    wait_queued(&admission, 1);
    assert_eq!(
        admission.state().active_keys.len(),
        1,
        "waiting multi-key request holds no subset"
    );
    let (single_tx, single_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let single = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("a");
        single_tx.send(()).unwrap();
    });
    wait_queued(&admission, 2);
    let worker = Arc::clone(&admission);
    let unrelated = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("c");
    });
    unrelated.join().unwrap();
    assert!(single_rx.try_recv().is_err());
    drop(initial);
    multi_rx.recv_timeout(DEADLINE).unwrap();
    assert_eq!(admission.state().active_keys.len(), 2);
    assert!(single_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    single_rx.recv_timeout(DEADLINE).unwrap();
    multi.join().unwrap();
    single.join().unwrap();
    assert_drained(&admission);
}

#[test]
fn keyed_admission_global_holder_excludes_every_resource() {
    let admission = Arc::new(KeyedAdmission::default());
    let global = admission.lock().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let worker = Arc::clone(&admission);
    let conversation = std::thread::spawn(move || {
        let _guard = worker.lock_conversation("conversation");
        entered_tx.send(()).unwrap();
    });
    wait_queued(&admission, 1);
    assert!(entered_rx.try_recv().is_err());
    drop(global);
    entered_rx.recv_timeout(DEADLINE).unwrap();
    conversation.join().unwrap();
    assert_drained(&admission);
}

#[test]
fn keyed_admission_nested_acquisition_is_rejected_without_poisoning_owner() {
    let admission = KeyedAdmission::default();
    let owner = admission.lock_conversation("a");
    for global in [false, true] {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if global {
                let _guard = admission.lock().unwrap();
            } else {
                let _guard = admission.lock_conversation("b");
            }
        }))
        .is_err());
    }
    assert_eq!(
        admission.state().active_keys,
        HashSet::from(["a".to_string()])
    );
    drop(owner);
    let later = admission.lock().unwrap();
    drop(later);
    assert_drained(&admission);
}

#[test]
fn keyed_admission_unwind_releases_exact_keys_and_does_not_retain_registry_entries() {
    let admission = KeyedAdmission::default();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = admission.lock_conversations(&["a".into(), "b".into()]);
        panic!("synthetic preparation failure");
    }))
    .is_err());
    assert_drained(&admission);
    for index in 0..1000 {
        let _guard = admission.lock_conversation(&format!("short-lived-{index}"));
    }
    assert_drained(&admission);
}

#[test]
fn keyed_admission_rejects_unknown_empty_resource_set() {
    let admission = KeyedAdmission::default();
    for keys in [Vec::new(), vec![String::new()]] {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = admission.lock_conversations(&keys);
        }))
        .is_err());
    }
    assert_drained(&admission);
}
