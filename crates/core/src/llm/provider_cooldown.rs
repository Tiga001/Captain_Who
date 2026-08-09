use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

const MAX_COOLDOWN_SCOPES: usize = 1_024;
const DEFAULT_RATE_LIMIT_COOLDOWN_MS: u64 = 2_000;

#[derive(Clone, Eq)]
struct CooldownKey([u8; 32]);

impl PartialEq for CooldownKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Hash for CooldownKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

struct CooldownEntry {
    until: Instant,
    generation: u64,
    in_flight_generation: Option<u64>,
    queued: usize,
    notify: Arc<Notify>,
}

#[derive(Default)]
struct CooldownRegistry {
    entries: HashMap<CooldownKey, CooldownEntry>,
    next_generation: u64,
}

/// RAII proof that this request owns the only half-open probe for its provider scope.
///
/// The key is an irreversible SHA-256 digest over endpoint origin, API style, and credential. It
/// has no Debug implementation and is never persisted or surfaced in events.
pub(super) struct ProviderCooldownPermit {
    key: CooldownKey,
    generation: Option<u64>,
}

impl ProviderCooldownPermit {
    fn complete(mut self) {
        release_cooldown_permit(&self.key, self.generation.take());
    }
}

impl Drop for ProviderCooldownPermit {
    fn drop(&mut self) {
        release_cooldown_permit(&self.key, self.generation.take());
    }
}

struct CooldownWaiterRegistration {
    key: CooldownKey,
    registered: bool,
}

impl Drop for CooldownWaiterRegistration {
    fn drop(&mut self) {
        if self.registered {
            cancel_cooldown_waiter(&self.key);
        }
    }
}

fn cooldown_registry() -> &'static Mutex<CooldownRegistry> {
    static REGISTRY: OnceLock<Mutex<CooldownRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(CooldownRegistry::default()))
}

fn cooldown_key(api_url: &str, api_token: &str, api_style: AgentApiStyle) -> CooldownKey {
    let mut hasher = Sha256::new();
    if let Ok(url) = reqwest::Url::parse(api_url.trim()) {
        hasher.update(url.scheme().as_bytes());
        hasher.update([0]);
        if let Some(host) = url.host_str() {
            hasher.update(host.as_bytes());
        }
        hasher.update([0]);
        if let Some(port) = url.port_or_known_default() {
            hasher.update(port.to_be_bytes());
        }
    } else {
        hasher.update(api_url.trim().as_bytes());
    }
    hasher.update([0]);
    hasher.update(match api_style {
        AgentApiStyle::OpenAiCompatible => b"openai".as_slice(),
        AgentApiStyle::AnthropicCompatible => b"anthropic".as_slice(),
    });
    hasher.update([0]);
    hasher.update(api_token.as_bytes());
    CooldownKey(hasher.finalize().into())
}

pub(super) async fn acquire_provider_cooldown(
    api_url: &str,
    api_token: &str,
    api_style: AgentApiStyle,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<ProviderCooldownPermit> {
    let key = cooldown_key(api_url, api_token, api_style);
    let mut waiter = CooldownWaiterRegistration {
        key: key.clone(),
        registered: false,
    };
    loop {
        cancellation_token.check()?;
        let wait = {
            let mut registry = cooldown_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = registry.entries.get_mut(&key) else {
                return Ok(ProviderCooldownPermit {
                    key,
                    generation: None,
                });
            };
            if !waiter.registered {
                entry.queued = entry.queued.saturating_add(1);
                waiter.registered = true;
            }
            let now = Instant::now();
            if now >= entry.until && entry.in_flight_generation.is_none() {
                entry.queued = entry.queued.saturating_sub(1);
                entry.in_flight_generation = Some(entry.generation);
                waiter.registered = false;
                return Ok(ProviderCooldownPermit {
                    key,
                    generation: Some(entry.generation),
                });
            }

            // Register the notification waiter before releasing the registry lock. In
            // particular, notify_waiters does not retain a permit for a future waiter.
            let mut notified = Box::pin(Arc::clone(&entry.notify).notified_owned());
            notified.as_mut().enable();
            (
                entry.in_flight_generation.is_none().then_some(entry.until),
                notified,
            )
        };

        if let Some(until) = wait.0 {
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(until)) => {}
                _ = wait.1 => {}
            }
        } else {
            // The deadline may already be in the past while a half-open probe is running. Only
            // its completion notification may release another request; sleeping would hot-loop.
            tokio::select! {
                _ = cancellation_token.cancelled() => return Err(AgentError::cancelled()),
                _ = wait.1 => {}
            }
        }
    }
}

pub(super) fn register_provider_cooldown(
    api_url: &str,
    api_token: &str,
    api_style: AgentApiStyle,
    delay: Duration,
) {
    let key = cooldown_key(api_url, api_token, api_style);
    let delay = delay.max(Duration::from_millis(1));
    let now = Instant::now();
    let new_until = now
        .checked_add(delay)
        .unwrap_or_else(|| now + Duration::from_secs(100_u64.saturating_mul(365 * 24 * 60 * 60)));
    let mut registry = cooldown_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.next_generation = registry.next_generation.wrapping_add(1).max(1);
    let generation = registry.next_generation;
    if registry.entries.len() >= MAX_COOLDOWN_SCOPES && !registry.entries.contains_key(&key) {
        if let Some(expired) = registry
            .entries
            .iter()
            .find(|(_, entry)| {
                entry.queued == 0
                    && entry.in_flight_generation.is_none()
                    && Instant::now() >= entry.until
            })
            .map(|(key, _)| key.clone())
        {
            registry.entries.remove(&expired);
        } else {
            // This process-local coordinator must never evict an active scope to remember a new
            // one. The individual request still retains its own retry budget.
            return;
        }
    }
    let entry = registry
        .entries
        .entry(key)
        .or_insert_with(|| CooldownEntry {
            until: Instant::now(),
            generation,
            in_flight_generation: None,
            queued: 0,
            notify: Arc::new(Notify::new()),
        });
    // Concurrent failures can report the same shared bucket. A later fallback/jitter registration
    // must never shorten an authoritative provider Retry-After deadline.
    entry.until = entry.until.max(new_until);
    entry.generation = generation;
    // A request from the pre-cooldown wave can report another 429 while the half-open probe is
    // still physically in flight. Preserve that ownership; the new generation may not probe
    // until the stale permit is dropped.
    entry.notify.notify_waiters();
}

pub(super) fn register_default_rate_limit_cooldown(
    api_url: &str,
    api_token: &str,
    api_style: AgentApiStyle,
    retry_after_ms: Option<u64>,
) {
    register_provider_cooldown(
        api_url,
        api_token,
        api_style,
        Duration::from_millis(retry_after_ms.unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN_MS)),
    );
}

pub(super) fn complete_provider_cooldown(permit: ProviderCooldownPermit) {
    permit.complete();
}

fn release_cooldown_permit(key: &CooldownKey, generation: Option<u64>) {
    let Some(generation) = generation else {
        return;
    };
    let mut registry = cooldown_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = registry.entries.get_mut(key) else {
        return;
    };
    if entry.in_flight_generation != Some(generation) {
        return;
    }
    entry.in_flight_generation = None;
    if entry.queued == 0 {
        registry.entries.remove(key);
    } else {
        // Drain pre-existing waiters one at a time after a successful probe so the recovery edge
        // does not become a new request burst.
        entry.notify.notify_one();
    }
}

fn cancel_cooldown_waiter(key: &CooldownKey) {
    let mut registry = cooldown_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = registry.entries.get_mut(key) else {
        return;
    };
    entry.queued = entry.queued.saturating_sub(1);
    if entry.in_flight_generation.is_none() && Instant::now() >= entry.until && entry.queued > 0 {
        entry.notify.notify_one();
    }
}

pub(super) fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_only_one_half_open_probe_at_a_time() {
        let url = "https://cooldown-fixture.invalid/v1/chat/completions";
        let token = "cooldown-fixture-token";
        register_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            Duration::from_millis(10),
        );

        let first = acquire_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            AgentCancellationToken::new(),
        );
        let second = acquire_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            AgentCancellationToken::new(),
        );
        tokio::pin!(first);
        tokio::pin!(second);

        let first_permit = tokio::time::timeout(Duration::from_millis(100), &mut first)
            .await
            .expect("first half-open probe should be released")
            .expect("first cooldown acquisition should succeed");
        assert!(tokio::time::timeout(Duration::from_millis(20), &mut second)
            .await
            .is_err());

        complete_provider_cooldown(first_permit);
        let second_permit = tokio::time::timeout(Duration::from_millis(100), &mut second)
            .await
            .expect("second request should be released after the probe")
            .expect("second cooldown acquisition should succeed");
        complete_provider_cooldown(second_permit);
    }

    #[tokio::test]
    async fn scopes_by_endpoint_and_credential_and_cancels_waits() {
        let url = "https://cooldown-scope-a.invalid/v1/chat/completions";
        let other_url = "https://cooldown-scope-b.invalid/v1/chat/completions";
        let token = "cooldown-scope-token-a";
        let other_token = "cooldown-scope-token-b";
        register_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            Duration::from_secs(10),
        );

        let first = tokio::time::timeout(
            Duration::from_millis(50),
            acquire_provider_cooldown(
                url,
                other_token,
                AgentApiStyle::OpenAiCompatible,
                AgentCancellationToken::new(),
            ),
        )
        .await
        .expect("a different credential must not share the cooldown")
        .expect("different-credential acquisition should succeed");
        let second = tokio::time::timeout(
            Duration::from_millis(50),
            acquire_provider_cooldown(
                other_url,
                token,
                AgentApiStyle::OpenAiCompatible,
                AgentCancellationToken::new(),
            ),
        )
        .await
        .expect("a different endpoint must not share the cooldown")
        .expect("different-endpoint acquisition should succeed");
        complete_provider_cooldown(first);
        complete_provider_cooldown(second);

        let cancellation = AgentCancellationToken::new();
        let waiting = acquire_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            cancellation.clone(),
        );
        let cancel = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancellation.cancel();
        });
        let result = tokio::time::timeout(Duration::from_millis(500), waiting)
            .await
            .expect("cooldown cancellation must be observed");
        cancel.await.unwrap();
        let error = match result {
            Ok(_) => panic!("cancelled cooldown unexpectedly acquired a permit"),
            Err(error) => error,
        };
        assert!(error.is_cancelled());
    }

    #[tokio::test]
    async fn dropping_probe_permit_releases_the_next_waiter() {
        let url = "https://cooldown-drop-fixture.invalid/v1/chat/completions";
        let token = "cooldown-drop-token";
        register_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            Duration::from_millis(5),
        );
        let first = acquire_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            AgentCancellationToken::new(),
        );
        let second = acquire_provider_cooldown(
            url,
            token,
            AgentApiStyle::OpenAiCompatible,
            AgentCancellationToken::new(),
        );
        tokio::pin!(first);
        tokio::pin!(second);

        let permit = tokio::time::timeout(Duration::from_millis(100), &mut first)
            .await
            .expect("first probe should start")
            .expect("first probe acquisition should succeed");
        assert!(tokio::time::timeout(Duration::from_millis(15), &mut second)
            .await
            .is_err());
        drop(permit);
        let next = tokio::time::timeout(Duration::from_millis(100), &mut second)
            .await
            .expect("dropping the permit must release the next waiter")
            .expect("second probe acquisition should succeed");
        complete_provider_cooldown(next);
    }
}
