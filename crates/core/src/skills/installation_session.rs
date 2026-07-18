//! Shared, bounded in-memory state for Skill source resolution and installation preparation.
//!
//! Resolution candidates and prepared installation snapshots live behind one
//! lock so selecting a candidate can move its exact bytes into a preparation
//! without a second acquisition, a TOCTOU window, or transient double
//! accounting.

use super::installation_service::SkillInstallationMutation;
use super::installation_workflow::{SkillInstallationPreparationRequest, SkillInstallationPreview};
use super::prepared_acquisition::PreparedSkillAcquisition;
use super::source_resolution::{
    SkillInstallationSourceLocator, SkillRegisteredSourceResolution, SkillSourceCandidateId,
    SkillSourceResolutionId,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS: usize = 16;
pub const DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES: usize = 256;
pub const DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES: usize = 256;
pub const DEFAULT_SKILL_SOURCE_RESOLUTION_TTL: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;

const DEFAULT_SKILL_SOURCE_RESOLVING_TTL: Duration = Duration::from_secs(2 * 60);
const MAX_SKILL_SOURCE_RESOLUTIONS: usize = 4_096;
const MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES: usize = 65_536;
const MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES: usize = 16_384;
const MAX_SKILL_SESSION_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_SKILL_SOURCE_RESOLUTION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationSessionConfig {
    max_resolutions: usize,
    max_resolution_tombstones: usize,
    max_resolution_candidates: usize,
    max_snapshot_bytes: usize,
    resolution_ttl: Duration,
    resolving_ttl: Duration,
}

impl SkillInstallationSessionConfig {
    pub fn new(
        max_resolutions: usize,
        max_resolution_candidates: usize,
        max_snapshot_bytes: usize,
        resolution_ttl: Duration,
    ) -> Result<Self, SkillInstallationSessionConfigurationError> {
        if !(1..=MAX_SKILL_SOURCE_RESOLUTIONS).contains(&max_resolutions) {
            return Err(SkillInstallationSessionConfigurationError::new(format!(
                "max resolutions must be between 1 and {MAX_SKILL_SOURCE_RESOLUTIONS}",
            )));
        }
        if !(1..=MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES).contains(&max_resolution_candidates) {
            return Err(SkillInstallationSessionConfigurationError::new(format!(
                "max retained resolution candidates must be between 1 and {MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES}",
            )));
        }
        if max_snapshot_bytes == 0 || max_snapshot_bytes > MAX_SKILL_SESSION_SNAPSHOT_BYTES {
            return Err(SkillInstallationSessionConfigurationError::new(format!(
                "session snapshot budget must be between 1 and {MAX_SKILL_SESSION_SNAPSHOT_BYTES} bytes",
            )));
        }
        if resolution_ttl < Duration::from_millis(1)
            || resolution_ttl > MAX_SKILL_SOURCE_RESOLUTION_TTL
        {
            return Err(SkillInstallationSessionConfigurationError::new(
                "resolution TTL must be at least one millisecond and no more than 24 hours",
            ));
        }
        Ok(Self {
            max_resolutions,
            max_resolution_tombstones: DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES
                .max(max_resolutions),
            max_resolution_candidates,
            max_snapshot_bytes,
            resolution_ttl,
            resolving_ttl: DEFAULT_SKILL_SOURCE_RESOLVING_TTL,
        })
    }

    pub fn max_resolutions(&self) -> usize {
        self.max_resolutions
    }

    /// Maximum retained resolution records, including active slots reserved to become
    /// cancellation/consumption tombstones. This bound is independent of active concurrency.
    pub fn max_resolution_tombstones(&self) -> usize {
        self.max_resolution_tombstones
    }

    pub fn with_max_resolution_tombstones(
        mut self,
        max_resolution_tombstones: usize,
    ) -> Result<Self, SkillInstallationSessionConfigurationError> {
        if max_resolution_tombstones < self.max_resolutions
            || max_resolution_tombstones > MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES
        {
            return Err(SkillInstallationSessionConfigurationError::new(format!(
                "max resolution tombstones must be between {} and {MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES}",
                self.max_resolutions
            )));
        }
        self.max_resolution_tombstones = max_resolution_tombstones;
        Ok(self)
    }

    pub fn max_resolution_candidates(&self) -> usize {
        self.max_resolution_candidates
    }

    pub fn max_snapshot_bytes(&self) -> usize {
        self.max_snapshot_bytes
    }

    pub fn resolution_ttl(&self) -> Duration {
        self.resolution_ttl
    }

    pub(super) fn resolving_ttl(&self) -> Duration {
        self.resolving_ttl
    }

    #[cfg(test)]
    pub(super) fn with_resolving_ttl(mut self, ttl: Duration) -> Self {
        self.resolving_ttl = ttl;
        self
    }
}

impl Default for SkillInstallationSessionConfig {
    fn default() -> Self {
        Self {
            max_resolutions: DEFAULT_MAX_SKILL_SOURCE_RESOLUTIONS,
            max_resolution_tombstones: DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_TOMBSTONES,
            max_resolution_candidates: DEFAULT_MAX_SKILL_SOURCE_RESOLUTION_CANDIDATES,
            max_snapshot_bytes: DEFAULT_SKILL_SESSION_SNAPSHOT_BYTES,
            resolution_ttl: DEFAULT_SKILL_SOURCE_RESOLUTION_TTL,
            resolving_ttl: DEFAULT_SKILL_SOURCE_RESOLVING_TTL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationSessionConfigurationError {
    reason: String,
}

impl SkillInstallationSessionConfigurationError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl std::fmt::Display for SkillInstallationSessionConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl std::error::Error for SkillInstallationSessionConfigurationError {}

#[derive(Clone)]
pub struct SkillInstallationSessionStore {
    pub(super) inner: Arc<SkillInstallationSessionStoreInner>,
}

impl std::fmt::Debug for SkillInstallationSessionStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillInstallationSessionStore")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl Default for SkillInstallationSessionStore {
    fn default() -> Self {
        Self::new(SkillInstallationSessionConfig::default())
    }
}

impl SkillInstallationSessionStore {
    pub fn new(config: SkillInstallationSessionConfig) -> Self {
        Self::with_clock(config, Arc::new(SystemSessionClock::new()))
    }

    pub(super) fn with_clock(
        config: SkillInstallationSessionConfig,
        clock: Arc<dyn SessionClock>,
    ) -> Self {
        Self {
            inner: Arc::new(SkillInstallationSessionStoreInner {
                state: Mutex::new(InstallationSessionState::default()),
                changed: Condvar::new(),
                config,
                clock,
            }),
        }
    }

    pub fn config(&self) -> &SkillInstallationSessionConfig {
        &self.inner.config
    }

    pub(super) fn now(&self) -> SessionTime {
        self.inner.clock.now()
    }

    pub(super) fn lock(
        &self,
    ) -> Result<MutexGuard<'_, InstallationSessionState>, SessionRegistryError> {
        self.inner
            .state
            .lock()
            .map_err(|_| SessionRegistryError::Poisoned)
    }

    pub(super) fn wait<'a>(
        &self,
        state: MutexGuard<'a, InstallationSessionState>,
    ) -> Result<MutexGuard<'a, InstallationSessionState>, SessionRegistryError> {
        self.inner
            .changed
            .wait(state)
            .map_err(|_| SessionRegistryError::Poisoned)
    }

    pub(super) fn wait_until<'a>(
        &self,
        state: MutexGuard<'a, InstallationSessionState>,
        deadline: Duration,
    ) -> Result<MutexGuard<'a, InstallationSessionState>, SessionRegistryError> {
        let now = self.now().monotonic;
        let remaining = deadline.saturating_sub(now);
        self.inner
            .changed
            .wait_timeout(state, remaining)
            .map(|(state, _)| state)
            .map_err(|_| SessionRegistryError::Poisoned)
    }

    pub(super) fn notify_all(&self) {
        self.inner.changed.notify_all();
    }
}

pub(super) struct SkillInstallationSessionStoreInner {
    state: Mutex<InstallationSessionState>,
    changed: Condvar,
    config: SkillInstallationSessionConfig,
    clock: Arc<dyn SessionClock>,
}

#[derive(Default)]
pub(super) struct InstallationSessionState {
    pub(super) resolutions: BTreeMap<SkillSourceResolutionId, ResolutionSlot>,
    pub(super) preparations:
        BTreeMap<super::installation_workflow::SkillPreparationId, PreparationSlot>,
    pub(super) next_resolution_attempt: u64,
    pub(super) next_preparation_attempt: u64,
}

impl InstallationSessionState {
    pub(super) fn prune_expired(&mut self, now: Duration) {
        self.resolutions.retain(|_, slot| !slot.is_expired(now));
        self.preparations.retain(|_, slot| !slot.is_expired(now));
    }

    pub(super) fn reserved_snapshot_bytes(&self) -> usize {
        let resolutions = self
            .resolutions
            .values()
            .map(ResolutionSlot::reserved_snapshot_bytes)
            .sum::<usize>();
        self.preparations
            .values()
            .map(PreparationSlot::reserved_snapshot_bytes)
            .fold(resolutions, usize::saturating_add)
    }

    pub(super) fn retained_resolution_candidates(&self) -> usize {
        self.resolutions
            .values()
            .map(ResolutionSlot::retained_candidates)
            .sum()
    }

    pub(super) fn active_resolution_count(&self) -> usize {
        self.resolutions
            .values()
            .filter(|slot| slot.is_active())
            .count()
    }

    pub(super) fn retained_resolution_record_count(&self) -> usize {
        self.resolutions.len()
    }

    pub(super) fn next_resolution_attempt(&mut self) -> u64 {
        self.next_resolution_attempt = self.next_resolution_attempt.wrapping_add(1).max(1);
        self.next_resolution_attempt
    }

    pub(super) fn next_preparation_attempt(&mut self) -> u64 {
        self.next_preparation_attempt = self.next_preparation_attempt.wrapping_add(1).max(1);
        self.next_preparation_attempt
    }
}

pub(super) enum ResolutionSlot {
    Resolving {
        locator: SkillInstallationSourceLocator,
        attempt_id: u64,
        reserved_bytes: usize,
        expires_at: Duration,
    },
    Ready {
        locator: SkillInstallationSourceLocator,
        resolution: SkillRegisteredSourceResolution,
        candidates: BTreeMap<SkillSourceCandidateId, PreparedSkillAcquisition>,
        snapshot_bytes: usize,
        expires_at: Duration,
    },
    Consumed {
        locator: SkillInstallationSourceLocator,
        candidate_id: SkillSourceCandidateId,
        preparation_id: super::installation_workflow::SkillPreparationId,
        expires_at: Duration,
    },
    Cancelled {
        /// None is a cancellation fence recorded before a queued resolver binds its locator.
        locator: Option<SkillInstallationSourceLocator>,
        expires_at: Duration,
    },
}

impl ResolutionSlot {
    pub(super) fn locator(&self) -> Option<&SkillInstallationSourceLocator> {
        match self {
            Self::Resolving { locator, .. }
            | Self::Ready { locator, .. }
            | Self::Consumed { locator, .. } => Some(locator),
            Self::Cancelled { locator, .. } => locator.as_ref(),
        }
    }

    fn is_expired(&self, now: Duration) -> bool {
        match self {
            Self::Resolving { expires_at, .. }
            | Self::Ready { expires_at, .. }
            | Self::Consumed { expires_at, .. }
            | Self::Cancelled { expires_at, .. } => now >= *expires_at,
        }
    }

    fn is_active(&self) -> bool {
        matches!(self, Self::Resolving { .. } | Self::Ready { .. })
    }

    fn reserved_snapshot_bytes(&self) -> usize {
        match self {
            Self::Resolving { reserved_bytes, .. } => *reserved_bytes,
            Self::Ready { snapshot_bytes, .. } => *snapshot_bytes,
            Self::Consumed { .. } | Self::Cancelled { .. } => 0,
        }
    }

    fn retained_candidates(&self) -> usize {
        match self {
            Self::Ready { candidates, .. } => candidates.len(),
            _ => 0,
        }
    }
}

pub(super) enum PreparationSlot {
    Preparing {
        request: SkillInstallationPreparationRequest,
        attempt_id: u64,
        _started_at: Duration,
    },
    Ready {
        request: SkillInstallationPreparationRequest,
        preview: SkillInstallationPreview,
        acquisition: PreparedSkillAcquisition,
        snapshot_bytes: usize,
        expires_at: Duration,
    },
    Committing {
        request: SkillInstallationPreparationRequest,
        attempt_id: u64,
        snapshot_bytes: usize,
    },
    Committed {
        request: SkillInstallationPreparationRequest,
        preview: SkillInstallationPreview,
        mutation: SkillInstallationMutation,
        expires_at: Duration,
    },
    Cancelled {
        request: SkillInstallationPreparationRequest,
        expires_at: Duration,
    },
}

impl PreparationSlot {
    pub(super) fn request(&self) -> &SkillInstallationPreparationRequest {
        match self {
            Self::Preparing { request, .. }
            | Self::Ready { request, .. }
            | Self::Committing { request, .. }
            | Self::Committed { request, .. }
            | Self::Cancelled { request, .. } => request,
        }
    }

    fn is_expired(&self, now: Duration) -> bool {
        match self {
            Self::Ready { expires_at, .. }
            | Self::Committed { expires_at, .. }
            | Self::Cancelled { expires_at, .. } => now >= *expires_at,
            Self::Preparing { .. } | Self::Committing { .. } => false,
        }
    }

    pub(super) fn reserved_snapshot_bytes(&self) -> usize {
        match self {
            Self::Preparing { .. } => super::package::MAX_SKILL_PACKAGE_BYTES,
            Self::Ready { snapshot_bytes, .. } | Self::Committing { snapshot_bytes, .. } => {
                *snapshot_bytes
            }
            Self::Committed { .. } | Self::Cancelled { .. } => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionRegistryError {
    Poisoned,
}

#[derive(Clone, Copy)]
pub(super) struct SessionTime {
    pub(super) monotonic: Duration,
    pub(super) unix_ms: u64,
}

pub(super) trait SessionClock: Send + Sync {
    fn now(&self) -> SessionTime;
}

struct SystemSessionClock {
    started: Instant,
    started_unix_ms: u64,
}

impl SystemSessionClock {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            started_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(duration_millis)
                .unwrap_or(0),
        }
    }
}

impl SessionClock for SystemSessionClock {
    fn now(&self) -> SessionTime {
        let monotonic = self.started.elapsed();
        SessionTime {
            monotonic,
            unix_ms: self
                .started_unix_ms
                .saturating_add(duration_millis(monotonic)),
        }
    }
}

pub(super) fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
