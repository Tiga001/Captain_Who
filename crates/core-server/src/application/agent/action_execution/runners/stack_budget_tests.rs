use super::*;

fn assert_small_await_state<Args, F: std::future::Future>(_: impl FnOnce(Args) -> F) {
    // These futures are nested in the same poll chain as the Runtime and TLS certificate
    // verification. Before heap allocation at the boundaries, each embedded >100 KiB of
    // state in its caller and debug-build poll frames exhausted a default 2 MiB worker.
    const MAX_INLINE_AWAIT_STATE_BYTES: usize = 16 * 1024;
    assert!(
        std::mem::size_of::<F>() <= MAX_INLINE_AWAIT_STATE_BYTES,
        "approval await state uses {} bytes, exceeding its {MAX_INLINE_AWAIT_STATE_BYTES}-byte stack budget",
        std::mem::size_of::<F>(),
    );
}

#[test]
fn approval_execution_and_continuation_keep_inline_await_state_bounded() {
    // The closures are never invoked: infer the production future types without starting a
    // command, opening storage, or constructing an agent execution context.
    assert_small_await_state(
        |(service, record, call, guard, notifications): (&'static AgentService, _, _, _, _)| {
            service.run_command_execution(record, call, guard, notifications)
        },
    );
    assert_small_await_state(
        |(service, record, input, notifications, status, cancellation): (
            &'static AgentService,
            _,
            _,
            _,
            _,
            _,
        )| {
            service.run_action_continuation(record, input, notifications, status, cancellation)
        },
    );
}
