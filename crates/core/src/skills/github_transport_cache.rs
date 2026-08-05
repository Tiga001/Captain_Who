//! Shared, bounded GitHub transport cache and same-key request coalescing.
//!
//! The wrapper is intentionally below source resolution and acquisition so
//! Settings and Agent installation share exactly the same ref, rate-limit and
//! archive state. Archive entries retain private immutable spools rather than
//! public paths or response bytes.

use super::github_acquisition::{
    GitHubAcquisitionTransport, GitHubArchive, GitHubArchiveRequest, GitHubCommit, GitHubReference,
    GitHubResolveRequest, GitHubTransportError,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const REFERENCE_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const ARCHIVE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const TRANSIENT_ERROR_CACHE_TTL: Duration = Duration::from_secs(2);
const MAX_RATE_LIMIT_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_REFERENCE_CACHE_ENTRIES: usize = 256;
const MAX_ARCHIVE_CACHE_ENTRIES: usize = 8;
const MAX_ARCHIVE_CACHE_BYTES: usize = 128 * 1024 * 1024;
const MAX_IN_FLIGHT_REQUESTS: usize = 64;

pub struct SharedGitHubTransport {
    inner: Arc<dyn GitHubAcquisitionTransport>,
    state: Mutex<CacheState>,
    changed: Condvar,
    config: CacheConfig,
}

impl fmt::Debug for SharedGitHubTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedGitHubTransport")
            .finish_non_exhaustive()
    }
}

impl SharedGitHubTransport {
    pub fn new(inner: Arc<dyn GitHubAcquisitionTransport>) -> Self {
        Self::with_config(inner, CacheConfig::default())
    }

    fn with_config(inner: Arc<dyn GitHubAcquisitionTransport>, config: CacheConfig) -> Self {
        Self {
            inner,
            state: Mutex::new(CacheState::default()),
            changed: Condvar::new(),
            config,
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, CacheState>, GitHubTransportError> {
        self.state
            .lock()
            .map_err(|_| GitHubTransportError::Unavailable)
    }

    fn wait_for_change<'a>(
        &self,
        state: MutexGuard<'a, CacheState>,
    ) -> Result<MutexGuard<'a, CacheState>, GitHubTransportError> {
        self.changed
            .wait(state)
            .map_err(|_| GitHubTransportError::Unavailable)
    }
}

impl GitHubAcquisitionTransport for SharedGitHubTransport {
    fn resolve_commit(
        &self,
        request: &GitHubResolveRequest,
    ) -> Result<GitHubCommit, GitHubTransportError> {
        if let GitHubReference::Commit(commit) = request.reference() {
            return Ok(commit.clone());
        }
        loop {
            let now = Instant::now();
            let mut state = self.lock_state()?;
            state.prune(now);
            let access = state.next_access();
            if let Some(entry) = state.references.get_mut(request) {
                entry.last_access = access;
                return entry.value.clone();
            }
            if state.resolving.contains(request) {
                drop(self.wait_for_change(state)?);
                continue;
            }
            if state.in_flight_count() >= self.config.max_in_flight_requests {
                return Err(GitHubTransportError::Unavailable);
            }
            state.resolving.insert(request.clone());
            drop(state);

            let result = catch_unwind(AssertUnwindSafe(|| self.inner.resolve_commit(request)))
                .unwrap_or(Err(GitHubTransportError::Unavailable));
            let now = Instant::now();
            let mut state = self.lock_state()?;
            state.resolving.remove(request);
            let expires_at = now
                .checked_add(cache_ttl_for_result(&result, self.config.reference_ttl))
                .unwrap_or(now);
            let access = state.next_access();
            state.insert_reference(
                request.clone(),
                CacheEntry::new(result.clone(), expires_at, access),
                self.config.max_reference_entries,
            );
            self.changed.notify_all();
            return result;
        }
    }

    fn download_archive(
        &self,
        request: &GitHubArchiveRequest,
    ) -> Result<GitHubArchive, GitHubTransportError> {
        loop {
            let now = Instant::now();
            let mut state = self.lock_state()?;
            state.prune(now);
            let access = state.next_access();
            if let Some(entry) = state.archives.get_mut(request) {
                entry.last_access = access;
                let cached = entry.value.clone();
                drop(state);
                match cached {
                    Ok(archive) if archive.verify_integrity().is_ok() => return Ok(archive),
                    Ok(_) => {
                        let mut state = self.lock_state()?;
                        state.archives.remove(request);
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
            if state.downloading.contains(request) {
                drop(self.wait_for_change(state)?);
                continue;
            }
            if state.in_flight_count() >= self.config.max_in_flight_requests {
                return Err(GitHubTransportError::Unavailable);
            }
            state.downloading.insert(request.clone());
            drop(state);

            let result = catch_unwind(AssertUnwindSafe(|| self.inner.download_archive(request)))
                .unwrap_or(Err(GitHubTransportError::Unavailable));
            let now = Instant::now();
            let mut state = self.lock_state()?;
            state.downloading.remove(request);
            let expires_at = now
                .checked_add(cache_ttl_for_result(&result, self.config.archive_ttl))
                .unwrap_or(now);
            let access = state.next_access();
            state.insert_archive(
                request.clone(),
                CacheEntry::new(result.clone(), expires_at, access),
                &self.config,
            );
            self.changed.notify_all();
            return result;
        }
    }
}

#[derive(Clone, Copy)]
struct CacheConfig {
    reference_ttl: Duration,
    archive_ttl: Duration,
    max_reference_entries: usize,
    max_archive_entries: usize,
    max_archive_bytes: usize,
    max_in_flight_requests: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            reference_ttl: REFERENCE_CACHE_TTL,
            archive_ttl: ARCHIVE_CACHE_TTL,
            max_reference_entries: MAX_REFERENCE_CACHE_ENTRIES,
            max_archive_entries: MAX_ARCHIVE_CACHE_ENTRIES,
            max_archive_bytes: MAX_ARCHIVE_CACHE_BYTES,
            max_in_flight_requests: MAX_IN_FLIGHT_REQUESTS,
        }
    }
}

#[derive(Clone)]
struct CacheEntry<T> {
    value: T,
    expires_at: Instant,
    last_access: u64,
}

impl<T> CacheEntry<T> {
    fn new(value: T, expires_at: Instant, last_access: u64) -> Self {
        Self {
            value,
            expires_at,
            last_access,
        }
    }
}

#[derive(Default)]
struct CacheState {
    references:
        HashMap<GitHubResolveRequest, CacheEntry<Result<GitHubCommit, GitHubTransportError>>>,
    archives:
        HashMap<GitHubArchiveRequest, CacheEntry<Result<GitHubArchive, GitHubTransportError>>>,
    resolving: HashSet<GitHubResolveRequest>,
    downloading: HashSet<GitHubArchiveRequest>,
    access_counter: u64,
}

impl CacheState {
    fn next_access(&mut self) -> u64 {
        self.access_counter = self.access_counter.wrapping_add(1);
        self.access_counter
    }

    fn in_flight_count(&self) -> usize {
        self.resolving.len().saturating_add(self.downloading.len())
    }

    fn prune(&mut self, now: Instant) {
        self.references.retain(|_, entry| entry.expires_at > now);
        self.archives.retain(|_, entry| entry.expires_at > now);
    }

    fn insert_reference(
        &mut self,
        key: GitHubResolveRequest,
        entry: CacheEntry<Result<GitHubCommit, GitHubTransportError>>,
        max_entries: usize,
    ) {
        self.references.insert(key, entry);
        while self.references.len() > max_entries {
            let Some(oldest) = self
                .references
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.references.remove(&oldest);
        }
    }

    fn insert_archive(
        &mut self,
        key: GitHubArchiveRequest,
        entry: CacheEntry<Result<GitHubArchive, GitHubTransportError>>,
        config: &CacheConfig,
    ) {
        self.archives.insert(key, entry);
        while self.archives.len() > config.max_archive_entries
            || self.archive_bytes() > config.max_archive_bytes
        {
            let Some(oldest) = self
                .archives
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.archives.remove(&oldest);
        }
    }

    fn archive_bytes(&self) -> usize {
        self.archives
            .values()
            .filter_map(|entry| entry.value.as_ref().ok())
            .fold(0usize, |total, archive| total.saturating_add(archive.len()))
    }
}

fn cache_ttl_for_result<T>(
    result: &Result<T, GitHubTransportError>,
    success_ttl: Duration,
) -> Duration {
    match result {
        Ok(_) => success_ttl,
        Err(GitHubTransportError::RateLimited { retry_after, .. }) => retry_after
            .unwrap_or(TRANSIENT_ERROR_CACHE_TTL)
            .min(MAX_RATE_LIMIT_CACHE_TTL),
        Err(_) => TRANSIENT_ERROR_CACHE_TTL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::GitHubRepository;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    struct CountingTransport {
        archive_calls: AtomicUsize,
        resolve_calls: AtomicUsize,
        archive: GitHubArchive,
    }

    impl GitHubAcquisitionTransport for CountingTransport {
        fn resolve_commit(
            &self,
            _request: &GitHubResolveRequest,
        ) -> Result<GitHubCommit, GitHubTransportError> {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            GitHubCommit::parse(COMMIT).map_err(|_| GitHubTransportError::InvalidResponse)
        }

        fn download_archive(
            &self,
            _request: &GitHubArchiveRequest,
        ) -> Result<GitHubArchive, GitHubTransportError> {
            self.archive_calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            Ok(self.archive.clone())
        }
    }

    fn requests() -> (GitHubResolveRequest, GitHubArchiveRequest) {
        let repository = GitHubRepository::parse("openai", "skills").unwrap();
        let commit = GitHubCommit::parse(COMMIT).unwrap();
        (
            GitHubResolveRequest::new(repository.clone(), GitHubReference::named("main").unwrap()),
            GitHubArchiveRequest::new(repository, commit),
        )
    }

    #[test]
    fn concurrent_identical_requests_share_resolution_and_archive_download() {
        let inner = Arc::new(CountingTransport {
            archive_calls: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            archive: GitHubArchive::from_bytes(b"not-a-zip-but-a-valid-cached-spool").unwrap(),
        });
        let shared = Arc::new(SharedGitHubTransport::new(inner.clone()));
        let (resolve, archive) = requests();
        thread::scope(|scope| {
            for _ in 0..8 {
                let shared = shared.clone();
                let resolve = resolve.clone();
                let archive = archive.clone();
                scope.spawn(move || {
                    shared.resolve_commit(&resolve).unwrap();
                    shared.download_archive(&archive).unwrap();
                });
            }
        });
        assert_eq!(inner.resolve_calls.load(Ordering::SeqCst), 1);
        assert_eq!(inner.archive_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn immutable_commit_resolution_never_reaches_the_inner_transport() {
        let inner = Arc::new(CountingTransport {
            archive_calls: AtomicUsize::new(0),
            resolve_calls: AtomicUsize::new(0),
            archive: GitHubArchive::from_bytes(b"archive").unwrap(),
        });
        let shared = SharedGitHubTransport::new(inner.clone());
        let repository = GitHubRepository::parse("openai", "skills").unwrap();
        let request =
            GitHubResolveRequest::new(repository, GitHubReference::commit(COMMIT).unwrap());
        assert_eq!(shared.resolve_commit(&request).unwrap().as_str(), COMMIT);
        assert_eq!(inner.resolve_calls.load(Ordering::SeqCst), 0);
    }
}
