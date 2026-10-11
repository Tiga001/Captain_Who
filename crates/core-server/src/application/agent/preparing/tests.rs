use super::*;
use std::sync::Barrier;

fn register(registry: &Arc<PreparingRegistry>, conversation: &str, run: &str) -> PreparingGuard {
    registry
        .register(
            conversation,
            Some("project"),
            run,
            &format!("assistant-{run}"),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap()
}

#[test]
fn preparing_same_conversation_has_one_owner_while_independent_preparations_coexist() {
    let registry = Arc::new(PreparingRegistry::default());
    let barrier = Barrier::new(2);
    let winners = std::thread::scope(|scope| {
        ["first", "second"]
            .map(|run| {
                let registry = &registry;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    registry.register(
                        "conversation",
                        Some("project"),
                        run,
                        "assistant",
                        AgentCancellationToken::new(),
                        None,
                    )
                })
            })
            .into_iter()
            .filter_map(|thread| thread.join().unwrap().ok())
            .collect::<Vec<_>>()
    });
    assert_eq!(winners.len(), 1);
    let independent = register(&registry, "independent", "independent-run");
    assert_eq!(registry.snapshot().len(), 2);
    assert!(winners[0].validate().is_ok());
    assert!(independent.validate().is_ok());
}

#[test]
fn preparing_snapshot_retains_authoritative_owner_revision_and_phase() {
    let registry = Arc::new(PreparingRegistry::default());
    let preparation = register(&registry, "conversation", "run");
    preparation.set_revision(Some(71)).unwrap();
    let owner = registry.snapshot_run("run").unwrap();
    assert_eq!(owner.token, *preparation.token());
    assert_eq!(owner.project_id.as_deref(), Some("project"));
    assert_eq!(owner.assistant_message_id, "assistant-run");
    assert_eq!(owner.source_revision, Some(71));
    assert_eq!(owner.phase, PreparingPhase::Preparing);
    preparation.mark_committing().unwrap();
    assert_eq!(
        registry.snapshot_run("run").unwrap().phase,
        PreparingPhase::Committing
    );
    assert!(preparation.set_revision(Some(72)).is_err());
    assert!(registry.cancel_run("run"));
    assert!(preparation.validate().is_err());
    assert!(owner.cancellation.is_cancelled());
    // Cancellation does not release an executing synchronous job or its registration.
    assert!(!registry.wait_scope_idle(
        &PreparingScope::Conversation("conversation".into()),
        Duration::ZERO
    ));
    drop(preparation);
    assert!(registry.wait_scope_idle(
        &PreparingScope::Conversation("conversation".into()),
        Duration::ZERO
    ));
}

#[test]
fn preparing_old_generation_and_boot_cannot_validate_or_retire_successor() {
    let registry = Arc::new(PreparingRegistry::default());
    let first = register(&registry, "conversation", "first");
    let old_token = first.token().clone();
    // Simulate retiring an owner through another authoritative lifecycle boundary. Its late RAII
    // drop must not erase the replacement installed for that Conversation.
    registry.state().entries.remove("conversation");
    let successor = register(&registry, "conversation", "next");
    assert_ne!(old_token.generation, successor.token().generation);
    assert!(registry.validate(&old_token).is_err());
    drop(first);
    assert!(successor.validate().is_ok());
    let restarted = Arc::new(PreparingRegistry::default());
    let replacement = register(&restarted, "conversation", "next");
    assert_ne!(replacement.token().boot_id, successor.token().boot_id);
    assert!(restarted.validate(successor.token()).is_err());
}

#[test]
fn preparing_deletion_fences_registration_and_cancels_only_its_scope() {
    let registry = Arc::new(PreparingRegistry::default());
    let target = register(&registry, "target", "target-run");
    let sibling = register(&registry, "sibling", "sibling-run");
    let external = registry
        .register(
            "external",
            Some("elsewhere"),
            "external-run",
            "assistant",
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
    let scope = PreparingScope::Conversation("target".into());
    assert_eq!(
        registry.begin_delete_scope(&scope).unwrap(),
        vec!["target-run"]
    );
    assert!(target.validate().is_err());
    assert!(sibling.validate().is_ok());
    assert!(external.validate().is_ok());
    assert!(registry.begin_delete_scope(&scope).is_err());
    drop(target);
    assert!(registry
        .register(
            "target",
            Some("project"),
            "new-run",
            "assistant",
            AgentCancellationToken::new(),
            None
        )
        .is_err());
    registry.clear_delete_scope(&scope);
    let next = register(&registry, "target", "new-run");
    let project = PreparingScope::Project("project".into());
    let mut cancelled = registry.begin_delete_scope(&project).unwrap();
    cancelled.sort();
    assert_eq!(cancelled, vec!["new-run", "sibling-run"]);
    assert!(next.validate().is_err());
    assert!(sibling.validate().is_err());
    assert!(external.validate().is_ok());
    registry.clear_delete_scope(&project);
    assert!(
        next.validate().is_err(),
        "clearing a failed deletion must not revive cancelled generation"
    );
}

#[test]
fn preparing_scope_drain_wait_does_not_block_other_conversation_registration() {
    let registry = Arc::new(PreparingRegistry::default());
    let work = register(&registry, "slow", "slow-run");
    let scope = PreparingScope::Conversation("slow".into());
    assert_eq!(registry.cancel_scope(&scope), vec!["slow-run"]);
    assert!(!registry.wait_scope_idle(&scope, Duration::from_millis(1)));
    let barrier = Barrier::new(2);
    std::thread::scope(|threads| {
        let waiter = threads.spawn(|| {
            barrier.wait();
            registry.wait_scope_idle(&scope, Duration::from_secs(2))
        });
        barrier.wait();
        let unrelated = register(&registry, "unrelated", "unrelated-run");
        assert!(unrelated.validate().is_ok());
        assert!(work.validate().is_err());
        drop(work); // Actual completion, not cancellation, releases the synchronous owner.
        assert!(waiter.join().unwrap());
    });
}

#[tokio::test]
async fn preparing_shutdown_waits_for_real_retirement_without_losing_wakeup() {
    for _ in 0..32 {
        let registry = Arc::new(PreparingRegistry::default());
        let preparation = register(&registry, "conversation", "run");
        assert_eq!(registry.begin_shutdown(), vec!["run"]);
        assert!(preparation.validate().is_err());
        assert!(registry
            .register(
                "new",
                None,
                "new-run",
                "assistant",
                AgentCancellationToken::new(),
                None
            )
            .is_err());
        assert!(!registry.wait_idle(Duration::ZERO).await);
        let retire = tokio::spawn(async move {
            tokio::task::yield_now().await;
            drop(preparation);
        });
        assert!(registry.wait_idle(Duration::from_secs(1)).await);
        retire.await.unwrap();
        assert!(registry.snapshot().is_empty());
    }
}

#[test]
fn preparing_run_identity_cannot_alias_a_different_conversation() {
    let registry = Arc::new(PreparingRegistry::default());
    let _original = register(&registry, "first", "run");
    assert!(registry
        .register(
            "second",
            None,
            "run",
            "assistant",
            AgentCancellationToken::new(),
            None
        )
        .is_err());
    assert!(!registry.cancel_run("missing"));
    assert_eq!(
        registry.snapshot_run("run").unwrap().token.conversation_id,
        "first"
    );
}
