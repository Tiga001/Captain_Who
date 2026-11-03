use super::*;
use mycopilot_protocol_rs::{
    ExecutionAccessReason, SetExecutionAccessInput, SetExecutionAccessOutput,
};

type QueuedExecutionAccess = Result<ExecutionAccessToken, ExecutionAccessDenied>;
thread_local! {
    /// Synchronous RPC handlers run on a bounded blocking worker. Scope the frozen admission
    /// token to that call, not to AgentService clones inherited by background continuations.
    static REQUEST_ACCESS: std::cell::RefCell<Vec<(usize, QueuedExecutionAccess)>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

struct RequestAccessScope;
impl Drop for RequestAccessScope {
    fn drop(&mut self) {
        REQUEST_ACCESS.with(|requests| {
            requests.borrow_mut().pop();
        });
    }
}

/// Process-only Host grant, never reconstructed from SQLite or attached to an Agent prompt.
#[derive(Default)]
pub(crate) struct ExecutionAccessState {
    revision: u64,
    identity_epoch: u64,
    denial_generation: u64,
    grant: Option<ExecutionAccessGrant>,
}

/// Frozen before preparation. Renewing the same account lease is harmless; observing a denial
/// or a different account epoch permanently invalidates work that has not yet been admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionAccessToken {
    identity_epoch: u64,
    denial_generation: u64,
}

struct ExecutionAccessGrant {
    input: SetExecutionAccessInput,
    received_at: i64,
    received_instant: Instant,
    invalidated: AtomicBool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionAccessDenied {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl ExecutionAccessDenied {
    pub(crate) fn signed_out() -> Self {
        Self {
            code: "ACCOUNT_LOGIN_REQUIRED",
            message: "请登录账号后再启动新的回合。",
        }
    }
    pub(crate) fn unavailable() -> Self {
        Self {
            code: "ACCOUNT_LICENSE_UNAVAILABLE",
            message: "使用许可尚未验证或本地授权已过期，本次回合未启动。",
        }
    }
    pub(crate) fn agent_error(self) -> AgentServiceError {
        AgentServiceError::structured(self.message, serde_json::json!({ "code": self.code }))
    }
}

impl ExecutionAccessState {
    fn apply(
        &mut self,
        input: SetExecutionAccessInput,
        now: i64,
        instant: Instant,
    ) -> SetExecutionAccessOutput {
        if input.revision > self.revision && input.identity_epoch >= self.identity_epoch {
            let previous_lease_expired = self
                .grant
                .as_ref()
                .is_some_and(|grant| grant.input.reason == ExecutionAccessReason::Allowed)
                && self.check_at(now, instant).is_err();
            if input.identity_epoch != self.identity_epoch
                || input.reason != ExecutionAccessReason::Allowed
                || previous_lease_expired
            {
                // There can be at most u64::MAX accepted revisions, so this cannot wrap.
                self.denial_generation += 1;
            }
            self.revision = input.revision;
            self.identity_epoch = input.identity_epoch;
            let invalidated = input.reason == ExecutionAccessReason::Allowed
                && (now < input.issued_at || now >= input.valid_until);
            self.grant = Some(ExecutionAccessGrant {
                input,
                received_at: now,
                received_instant: instant,
                invalidated: AtomicBool::new(invalidated),
            });
        }
        SetExecutionAccessOutput {
            revision: self.revision,
        }
    }

    pub(crate) fn check(&self) -> Result<(), ExecutionAccessDenied> {
        self.check_at(now_ms(), Instant::now())
    }

    fn capture(&self) -> Result<ExecutionAccessToken, ExecutionAccessDenied> {
        self.check()?;
        Ok(ExecutionAccessToken {
            identity_epoch: self.identity_epoch,
            denial_generation: self.denial_generation,
        })
    }

    fn check_token(&self, token: ExecutionAccessToken) -> Result<(), ExecutionAccessDenied> {
        self.check()?;
        if token.identity_epoch != self.identity_epoch
            || token.denial_generation != self.denial_generation
        {
            return Err(ExecutionAccessDenied::unavailable());
        }
        Ok(())
    }

    fn check_at(&self, now: i64, instant: Instant) -> Result<(), ExecutionAccessDenied> {
        let Some(grant) = &self.grant else {
            return Err(ExecutionAccessDenied::signed_out());
        };
        match grant.input.reason {
            ExecutionAccessReason::AccountSignedOut => {
                return Err(ExecutionAccessDenied::signed_out())
            }
            ExecutionAccessReason::LicenseRequired => {
                return Err(ExecutionAccessDenied {
                    code: "ACCOUNT_LICENSE_REQUIRED",
                    message: "账号使用许可无效或已到期，本次回合未启动。",
                })
            }
            ExecutionAccessReason::LicenseUnavailable => {
                return Err(ExecutionAccessDenied::unavailable())
            }
            ExecutionAccessReason::Allowed => {}
        }
        let elapsed = instant.saturating_duration_since(grant.received_instant);
        let remaining = grant
            .input
            .valid_until
            .saturating_sub(grant.received_at)
            .max(0) as u64;
        let expected_wall = grant
            .received_at
            .saturating_add(elapsed.as_millis().min(i64::MAX as u128) as i64);
        // Wall time enforces the actual license/cache expiry; Instant prevents rollback from
        // extending a Host lease. Observed clock jumps permanently invalidate this revision.
        if now < grant.input.issued_at
            || now >= grant.input.valid_until
            || elapsed >= Duration::from_millis(remaining)
            || now.abs_diff(expected_wall) > 2_000
        {
            grant.invalidated.store(true, Ordering::Release);
        }
        if grant.invalidated.load(Ordering::Acquire) {
            Err(ExecutionAccessDenied::unavailable())
        } else {
            Ok(())
        }
    }
}

impl AgentService {
    pub(crate) fn with_queued_execution_access<T>(
        &self,
        token: QueuedExecutionAccess,
        operation: impl FnOnce() -> T,
    ) -> T {
        let identity = Arc::as_ptr(&self.execution_access) as usize;
        REQUEST_ACCESS.with(|requests| requests.borrow_mut().push((identity, token)));
        let _scope = RequestAccessScope;
        operation()
    }

    pub(crate) fn set_execution_access(
        &self,
        input: SetExecutionAccessInput,
    ) -> Result<SetExecutionAccessOutput, String> {
        input.validate().map_err(str::to_string)?;
        let (output, changed, invalidate_predictions) =
            self.storage.publish_execution_access_boundary(|| {
                let mut access = self
                    .execution_access
                    .lock()
                    .map_err(|_| "Execution access state unavailable".to_string())?;
                let previous_revision = access.revision;
                let previous_generation = access.denial_generation;
                let output = access.apply(input, now_ms(), Instant::now());
                let changed = access.revision != previous_revision;
                let denial = access
                    .check()
                    .err()
                    .map(|denial| (denial.code, denial.message));
                // No storage operation is allowed while this short registry lock is held. Grant
                // publication and all unbound-attempt settlement share the surrounding writer job.
                Ok((
                    (
                        output,
                        changed,
                        access.denial_generation != previous_generation,
                    ),
                    denial,
                ))
            })?;
        if invalidate_predictions {
            self.clear_message_predictions();
        }
        if changed {
            self.workflow_readiness_changed(None);
        }
        Ok(output)
    }

    pub(crate) fn check_automation_execution_access(&self) -> Result<(), ExecutionAccessDenied> {
        self.execution_access
            .lock()
            .map_err(|_| ExecutionAccessDenied::unavailable())?
            .check()
    }

    pub(crate) fn capture_execution_access(
        &self,
    ) -> Result<ExecutionAccessToken, ExecutionAccessDenied> {
        let identity = Arc::as_ptr(&self.execution_access) as usize;
        if let Some(frozen) = REQUEST_ACCESS.with(|requests| {
            requests
                .borrow()
                .iter()
                .rev()
                .find(|(owner, _)| *owner == identity)
                .map(|(_, token)| *token)
        }) {
            let token = frozen?;
            self.check_execution_access_token(token)?;
            return Ok(token);
        }
        self.execution_access
            .lock()
            .map_err(|_| ExecutionAccessDenied::unavailable())?
            .capture()
    }

    /// Final admission invokes this inside the storage writer. Grant publication uses that same
    /// writer boundary, so releasing the registry lock cannot open a revoke-before-commit race.
    pub(crate) fn check_execution_access_token(
        &self,
        token: ExecutionAccessToken,
    ) -> Result<(), ExecutionAccessDenied> {
        self.execution_access
            .lock()
            .map_err(|_| ExecutionAccessDenied::unavailable())?
            .check_token(token)
    }

    /// The same writer boundary covers final Turn admission, so denial settlement cannot race a
    /// new binding. In particular, never hold the access registry while waiting for that writer.
    pub(crate) fn fail_disallowed_automation_runs(
        &self,
        run_id: Option<&str>,
        limit: usize,
    ) -> Result<usize, String> {
        self.storage
            .fail_unadmitted_automation_runs_at_execution_access_boundary(run_id, limit, || {
                let access = self
                    .execution_access
                    .lock()
                    .map_err(|_| "Execution access state unavailable".to_string())?;
                Ok(access
                    .check()
                    .err()
                    .map(|denial| (denial.code, denial.message)))
            })
    }

    #[cfg(test)]
    pub(crate) fn new_authorized_for_test(storage: Arc<StorageService>) -> Self {
        let service = Self::new(storage);
        service.grant_execution_access_for_test();
        service
    }

    #[cfg(test)]
    pub(crate) fn grant_execution_access_for_test(&self) {
        let now = now_ms();
        let current = self.execution_access.lock().unwrap();
        let revision = current.revision + 1;
        let identity_epoch = current.identity_epoch;
        drop(current);
        self.set_execution_access(SetExecutionAccessInput {
            revision,
            identity_epoch,
            reason: ExecutionAccessReason::Allowed,
            issued_at: now,
            valid_until: now + 60_000,
        })
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_account_token_is_scoped_to_one_handler_and_survives_neither_relogin_nor_panic() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &directory.path().join("queued.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let service = AgentService::new_authorized_for_test(storage);
        let queued = service.capture_execution_access();
        service
            .set_execution_access(live_snapshot(2, 1, ExecutionAccessReason::AccountSignedOut))
            .unwrap();
        service
            .set_execution_access(live_snapshot(3, 2, ExecutionAccessReason::Allowed))
            .unwrap();
        assert!(service.capture_execution_access().is_ok());
        service.with_queued_execution_access(queued, || {
            assert!(service.capture_execution_access().is_err());
            let background = service.clone();
            assert!(
                std::thread::spawn(move || background.capture_execution_access())
                    .join()
                    .unwrap()
                    .is_ok()
            );
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.with_queued_execution_access(queued, || panic!("handler fixture"));
        }))
        .is_err());
        assert!(
            service.capture_execution_access().is_ok(),
            "worker reuse must not inherit a retired request"
        );
        service.with_queued_execution_access(Err(ExecutionAccessDenied::signed_out()), || {
            assert_eq!(
                service.capture_execution_access().unwrap_err().code,
                "ACCOUNT_LOGIN_REQUIRED"
            );
        });
    }

    fn snapshot(
        revision: u64,
        identity_epoch: u64,
        reason: ExecutionAccessReason,
    ) -> SetExecutionAccessInput {
        SetExecutionAccessInput {
            revision,
            identity_epoch,
            reason,
            issued_at: 100_000,
            valid_until: if reason == ExecutionAccessReason::Allowed {
                160_000
            } else {
                100_000
            },
        }
    }

    fn live_snapshot(
        revision: u64,
        identity_epoch: u64,
        reason: ExecutionAccessReason,
    ) -> SetExecutionAccessInput {
        let now = now_ms();
        SetExecutionAccessInput {
            revision,
            identity_epoch,
            reason,
            issued_at: now,
            valid_until: if reason == ExecutionAccessReason::Allowed {
                now + 60_000
            } else {
                now
            },
        }
    }

    #[test]
    fn execution_access_token_survives_renewal_but_not_denial_or_account_change() {
        let mut state = ExecutionAccessState::default();
        state.apply(
            live_snapshot(1, 0, ExecutionAccessReason::Allowed),
            now_ms(),
            Instant::now(),
        );
        let initial = state.capture().unwrap();
        state.apply(
            live_snapshot(2, 0, ExecutionAccessReason::Allowed),
            now_ms(),
            Instant::now(),
        );
        assert!(state.check_token(initial).is_ok());
        // Stale Host messages cannot invalidate a current preparation either.
        state.apply(
            live_snapshot(1, 0, ExecutionAccessReason::AccountSignedOut),
            now_ms(),
            Instant::now(),
        );
        assert!(state.check_token(initial).is_ok());
        state.apply(
            live_snapshot(3, 0, ExecutionAccessReason::AccountSignedOut),
            now_ms(),
            Instant::now(),
        );
        state.apply(
            live_snapshot(4, 0, ExecutionAccessReason::Allowed),
            now_ms(),
            Instant::now(),
        );
        assert!(state.check_token(initial).is_err());
        let after_login = state.capture().unwrap();
        state.apply(
            live_snapshot(5, 1, ExecutionAccessReason::Allowed),
            now_ms(),
            Instant::now(),
        );
        assert!(state.check_token(after_login).is_err());
        assert!(state.capture().is_ok());
    }

    #[test]
    fn execution_access_expired_preparation_token_cannot_be_revived_by_late_renewal() {
        let mut state = ExecutionAccessState::default();
        let now = now_ms();
        let instant = Instant::now();
        state.apply(
            live_snapshot(1, 0, ExecutionAccessReason::Allowed),
            now,
            instant,
        );
        let initial = state.capture().unwrap();
        assert!(state
            .check_at(now + 60_001, instant + Duration::from_millis(60_001))
            .is_err());
        state.apply(
            live_snapshot(2, 0, ExecutionAccessReason::Allowed),
            now_ms(),
            Instant::now(),
        );
        assert!(state.check_token(initial).is_err());
        assert!(state.capture().is_ok());
    }

    #[test]
    fn execution_access_publication_waits_for_writer_without_holding_registry() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &fixture.path().join("storage.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
        let initial = service.capture_execution_access().unwrap();
        let (entered_send, entered_receive) = std::sync::mpsc::sync_channel(1);
        let (release_send, release_receive) = std::sync::mpsc::sync_channel(1);
        let holder_storage = Arc::clone(&storage);
        let holder = std::thread::spawn(move || {
            holder_storage.publish_execution_access_boundary(move || {
                entered_send.send(()).unwrap();
                release_receive.recv().unwrap();
                Ok(((), None))
            })
        });
        entered_receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let revoke_service = service.clone();
        let (started_send, started_receive) = std::sync::mpsc::sync_channel(1);
        let (finished_send, finished_receive) = std::sync::mpsc::sync_channel(1);
        let revoker = std::thread::spawn(move || {
            started_send.send(()).unwrap();
            let result = revoke_service.set_execution_access(live_snapshot(
                2,
                1,
                ExecutionAccessReason::AccountSignedOut,
            ));
            finished_send.send(result).unwrap();
        });
        started_receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        // A pending revocation is not an ACK; the current grant remains readable while its
        // control job waits. The old implementation took this lock before waiting for SQLite.
        assert!(finished_receive
            .recv_timeout(Duration::from_millis(30))
            .is_err());
        assert!(service.execution_access.try_lock().is_ok());
        assert!(service.check_execution_access_token(initial).is_ok());
        release_send.send(()).unwrap();
        holder.join().unwrap().unwrap();
        finished_receive
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        revoker.join().unwrap();
        assert!(service.check_execution_access_token(initial).is_err());
    }

    #[test]
    fn execution_access_defaults_denied_and_all_reasons_have_stable_codes() {
        let now = Instant::now();
        let mut state = ExecutionAccessState::default();
        assert_eq!(
            state.check_at(100_000, now).unwrap_err().code,
            "ACCOUNT_LOGIN_REQUIRED"
        );
        for (revision, reason, code) in [
            (
                1,
                ExecutionAccessReason::LicenseRequired,
                "ACCOUNT_LICENSE_REQUIRED",
            ),
            (
                2,
                ExecutionAccessReason::LicenseUnavailable,
                "ACCOUNT_LICENSE_UNAVAILABLE",
            ),
            (
                3,
                ExecutionAccessReason::AccountSignedOut,
                "ACCOUNT_LOGIN_REQUIRED",
            ),
        ] {
            state.apply(snapshot(revision, 0, reason), 100_000, now);
            assert_eq!(state.check_at(100_000, now).unwrap_err().code, code);
        }
    }

    #[test]
    fn execution_access_fences_both_revision_and_account_epoch() {
        let now = Instant::now();
        let mut state = ExecutionAccessState::default();
        state.apply(snapshot(1, 0, ExecutionAccessReason::Allowed), 100_000, now);
        assert!(state.check_at(100_000, now).is_ok());
        state.apply(
            snapshot(3, 1, ExecutionAccessReason::AccountSignedOut),
            100_000,
            now,
        );
        for (revision, epoch) in [(2, 0), (3, 1), (4, 0)] {
            assert_eq!(
                state
                    .apply(
                        snapshot(revision, epoch, ExecutionAccessReason::Allowed),
                        100_000,
                        now
                    )
                    .revision,
                3
            );
            assert_eq!(
                state.check_at(100_000, now).unwrap_err().code,
                "ACCOUNT_LOGIN_REQUIRED"
            );
        }
        state.apply(snapshot(4, 1, ExecutionAccessReason::Allowed), 100_000, now);
        assert!(state.check_at(100_000, now).is_ok());
    }

    #[test]
    fn execution_access_expires_at_wall_or_monotonic_deadline() {
        let now = Instant::now();
        for (wall, elapsed) in [(160_000, 60_000), (159_999, 60_000), (160_000, 59_999)] {
            let mut state = ExecutionAccessState::default();
            state.apply(snapshot(1, 0, ExecutionAccessReason::Allowed), 100_000, now);
            assert!(state
                .check_at(159_999, now + Duration::from_millis(59_999))
                .is_ok());
            assert_eq!(
                state
                    .check_at(wall, now + Duration::from_millis(elapsed))
                    .unwrap_err()
                    .code,
                "ACCOUNT_LICENSE_UNAVAILABLE"
            );
        }
    }

    #[test]
    fn execution_access_rejects_clock_jumps_permanently_for_same_revision() {
        let now = Instant::now();
        for wall in [99_999, 107_000, 120_000] {
            let mut state = ExecutionAccessState::default();
            state.apply(snapshot(1, 0, ExecutionAccessReason::Allowed), 100_000, now);
            assert!(state.check_at(wall, now + Duration::from_secs(10)).is_err());
            assert!(
                state
                    .check_at(110_000, now + Duration::from_secs(10))
                    .is_err(),
                "Returning wall time cannot resurrect a lease"
            );
        }
    }

    #[test]
    fn execution_access_delay_does_not_extend_a_received_lease() {
        let now = Instant::now();
        let mut state = ExecutionAccessState::default();
        state.apply(snapshot(1, 0, ExecutionAccessReason::Allowed), 150_000, now);
        assert!(state
            .check_at(159_999, now + Duration::from_millis(9_999))
            .is_ok());
        assert!(state
            .check_at(160_000, now + Duration::from_secs(10))
            .is_err());
        for received_at in [99_999, 160_000, 170_000] {
            let mut state = ExecutionAccessState::default();
            state.apply(
                snapshot(1, 0, ExecutionAccessReason::Allowed),
                received_at,
                now,
            );
            assert!(state.check_at(received_at, now).is_err());
        }
    }

    #[test]
    fn execution_access_restart_does_not_restore_previous_in_memory_grant() {
        let fixture = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open_with_model_credentials(
                &fixture.path().join("storage.sqlite"),
                Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
            )
            .unwrap(),
        );
        let service =
            AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
        service.grant_execution_access_for_test();
        assert!(service.check_automation_execution_access().is_ok());
        drop(service);
        let restarted = AgentService::try_new_deferred_startup_reconciliation(storage).unwrap();
        assert_eq!(
            restarted
                .check_automation_execution_access()
                .unwrap_err()
                .code,
            "ACCOUNT_LOGIN_REQUIRED"
        );
    }
}
