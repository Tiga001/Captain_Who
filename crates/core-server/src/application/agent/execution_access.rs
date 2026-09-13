use super::*;
use mycopilot_protocol_rs::{
    ExecutionAccessReason, SetExecutionAccessInput, SetExecutionAccessOutput,
};

/// Process-only Host grant, never reconstructed from SQLite or attached to an Agent prompt.
#[derive(Default)]
pub(crate) struct ExecutionAccessState {
    revision: u64,
    identity_epoch: u64,
    grant: Option<ExecutionAccessGrant>,
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
            message: "请登录账号后再启动新的自动化回合。",
        }
    }
    fn unavailable() -> Self {
        Self {
            code: "ACCOUNT_LICENSE_UNAVAILABLE",
            message: "使用许可尚未验证或本地授权已过期，本次自动化未启动。",
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
                    message: "账号使用许可无效或已到期，本次自动化未启动。",
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
    pub(crate) fn set_execution_access(
        &self,
        input: SetExecutionAccessInput,
    ) -> Result<SetExecutionAccessOutput, String> {
        input.validate().map_err(str::to_string)?;
        let mut access = self
            .execution_access
            .lock()
            .map_err(|_| "Execution access state unavailable".to_string())?;
        let output = access.apply(input, now_ms(), Instant::now());
        if let Err(denial) = access.check() {
            // Consume every already queued/unbound attempt while the denial is authoritative;
            // a subsequent quick login must not revive one that was waiting for a busy target.
            while self
                .storage
                .fail_unadmitted_automation_runs_for_execution_access(
                    None,
                    denial.code,
                    denial.message,
                    now_ms(),
                    100,
                )?
                == 100
            {}
        }
        Ok(output)
    }

    pub(crate) fn check_automation_execution_access(&self) -> Result<(), ExecutionAccessDenied> {
        self.execution_access
            .lock()
            .map_err(|_| ExecutionAccessDenied::unavailable())?
            .check()
    }

    /// The same lock covers final Turn admission, so denial settlement cannot race a new binding.
    pub(crate) fn fail_disallowed_automation_runs(
        &self,
        run_id: Option<&str>,
        limit: usize,
    ) -> Result<usize, String> {
        let access = self
            .execution_access
            .lock()
            .map_err(|_| "Execution access state unavailable".to_string())?;
        let Err(denial) = access.check() else {
            return Ok(0);
        };
        self.storage
            .fail_unadmitted_automation_runs_for_execution_access(
                run_id,
                denial.code,
                denial.message,
                now_ms(),
                limit,
            )
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
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
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
