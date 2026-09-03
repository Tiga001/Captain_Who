//! Secure, provider-neutral application credential storage.
//!
//! This module deliberately keeps credential bytes out of configuration records,
//! debug output, and error values. Persist only [`CredentialReference`] values in
//! application storage; the referenced secret remains in the operating system's
//! native credential store.

mod development_file;
#[cfg(target_os = "macos")]
mod macos_keychain;

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Mutex, MutexGuard},
};

use keyring::v1::{Entry, Error as KeyringError};
use uuid::Uuid;
use zeroize::Zeroize;

pub use development_file::DevelopmentFileCredentialStore;
#[cfg(target_os = "macos")]
pub use macos_keychain::NonInteractiveMacCredentialStore;

/// Stable service name used for image-provider credentials in the native store.
///
/// The per-credential username is an opaque [`CredentialReference`]. Keeping the
/// service stable lets configuration records rotate references without changing
/// the native-store namespace.
pub const IMAGE_GENERATION_CREDENTIAL_SERVICE: &str = "com.mycopilot.next.image-generation.v2";
/// Independent native-store namespace for language-model and search provider credentials.
pub const MODEL_PROVIDER_CREDENTIAL_SERVICE: &str = "com.mycopilot.next.model-provider.v1";

const OPAQUE_REFERENCE_PREFIX: &str = "application-credential/v1/";
const LEGACY_IMAGE_REFERENCE_PREFIX: &str = "image-generation/api-key/";
const OPAQUE_REFERENCE_UUID_BYTES: usize = 32;
const MAX_CREDENTIAL_SERVICE_BYTES: usize = 512;

/// Identifies the credential backend that owns a persisted reference.
///
/// The tag is part of every new reference. This makes backend migration fail
/// closed: a store never probes another backend (especially a legacy macOS
/// Keychain item) merely because SQLite still contains an older reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialStoreBackend {
    /// Unversioned references created before backend ownership was explicit.
    LegacySystemV1,
    /// Native OS credential storage used outside the dedicated macOS adapter.
    SystemV2,
    /// Non-interactive macOS Keychain adapter with a v2 service namespace.
    MacKeychainV2,
    /// Private, durable development-only file storage.
    DevelopmentFileV1,
    /// Deterministic in-process storage used by tests.
    InMemoryV1,
}

impl CredentialStoreBackend {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LegacySystemV1 => "legacy-system-v1",
            Self::SystemV2 => "system-v2",
            Self::MacKeychainV2 => "mac-keychain-v2",
            Self::DevelopmentFileV1 => "development-file-v1",
            Self::InMemoryV1 => "memory-v1",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "system-v2" => Some(Self::SystemV2),
            "mac-keychain-v2" => Some(Self::MacKeychainV2),
            "development-file-v1" => Some(Self::DevelopmentFileV1),
            "memory-v1" => Some(Self::InMemoryV1),
            _ => None,
        }
    }
}

/// An opaque, non-secret identifier for one credential-store entry.
///
/// This value is safe to persist. It never contains the credential itself.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialReference {
    value: String,
    backend: CredentialStoreBackend,
    opaque_id: String,
}

impl CredentialReference {
    /// Validates a reference loaded from persistent configuration.
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialStoreError> {
        let value = value.into();
        let (backend, opaque_id) = parse_reference(&value)?;
        let opaque_id = opaque_id.to_owned();
        Ok(Self {
            value,
            backend,
            opaque_id,
        })
    }

    /// Creates a fresh opaque reference suitable for credential rotation.
    ///
    /// A configuration transaction can write the new secret under this reference,
    /// commit the new reference with compare-and-swap, and only then delete the old
    /// reference. Failed configuration commits can safely delete the new reference.
    #[must_use]
    pub fn new_opaque() -> Self {
        Self::new_for_backend(CredentialStoreBackend::InMemoryV1)
    }

    /// Creates a new reference owned by `backend`.
    #[must_use]
    pub fn new_for_backend(backend: CredentialStoreBackend) -> Self {
        debug_assert_ne!(backend, CredentialStoreBackend::LegacySystemV1);
        let opaque_id = Uuid::new_v4().simple().to_string();
        Self {
            value: format!("{OPAQUE_REFERENCE_PREFIX}{}/{opaque_id}", backend.as_str()),
            backend,
            opaque_id,
        }
    }

    /// Creates a deterministic backend-owned reference from a fixed opaque identifier.
    ///
    /// This is intended for singleton Host keys whose lookup identity must survive process
    /// restarts without persisting a reference beside encrypted records. Callers must use a
    /// product-owned, non-user-derived lowercase hexadecimal identifier. Existing image-provider
    /// reference generation remains random and continues to use [`Self::new_for_backend`].
    pub fn from_stable_opaque_id(
        backend: CredentialStoreBackend,
        opaque_id: &str,
    ) -> Result<Self, CredentialStoreError> {
        if backend == CredentialStoreBackend::LegacySystemV1
            || opaque_id.len() != OPAQUE_REFERENCE_UUID_BYTES
            || !opaque_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CredentialStoreError::InvalidReference);
        }
        Ok(Self {
            value: format!("{OPAQUE_REFERENCE_PREFIX}{}/{opaque_id}", backend.as_str()),
            backend,
            opaque_id: opaque_id.to_string(),
        })
    }

    /// Returns the non-secret reference value for persistence or keyring lookup.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns the backend that owns this reference.
    #[must_use]
    pub const fn backend(&self) -> CredentialStoreBackend {
        self.backend
    }

    pub(crate) fn opaque_id(&self) -> &str {
        &self.opaque_id
    }
}

impl fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialReference([REDACTED])")
    }
}

/// A credential value whose memory is cleared when it is dropped.
///
/// The type intentionally does not implement `Clone`, `Display`, serialization,
/// or direct string access. Callers that must construct a provider header can use
/// [`CredentialSecret::with_secret_bytes`] for a narrowly scoped borrow.
pub struct CredentialSecret(String);

impl CredentialSecret {
    /// Wraps a non-empty secret without trimming or otherwise changing its bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, CredentialStoreError> {
        let value = value.into();
        if value.is_empty() {
            return Err(CredentialStoreError::InvalidSecret);
        }
        Ok(Self(value))
    }

    /// Temporarily exposes the credential bytes to a caller-provided closure.
    ///
    /// The borrowed slice cannot outlive this call. The closure must still avoid
    /// copying the value into logs, diagnostics, or long-lived request metadata.
    pub fn with_secret_bytes<T>(&self, expose: impl FnOnce(&[u8]) -> T) -> T {
        expose(self.0.as_bytes())
    }
}

impl fmt::Debug for CredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialSecret([REDACTED])")
    }
}

impl Drop for CredentialSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Outcome of an idempotent credential deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialDeleteOutcome {
    Deleted,
    NotFound,
}

/// Operation that failed against the credential backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreOperation {
    Open,
    Replace,
    Get,
    Delete,
}

impl fmt::Display for CredentialStoreOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Open => "open",
            Self::Replace => "replace",
            Self::Get => "get",
            Self::Delete => "delete",
        };
        formatter.write_str(name)
    }
}

/// Stable, redacted failures returned by [`CredentialStore`].
///
/// Platform errors are intentionally classified rather than embedded, because
/// native backend messages are not part of the public contract and may contain
/// values that should not enter logs or protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStoreError {
    InvalidReference,
    InvalidServiceName,
    InvalidSecret,
    BackendUnavailable { operation: CredentialStoreOperation },
    AccessDenied { operation: CredentialStoreOperation },
    Unsupported { operation: CredentialStoreOperation },
    CorruptedEntry { operation: CredentialStoreOperation },
    AmbiguousEntry { operation: CredentialStoreOperation },
    InvalidBackendInput { operation: CredentialStoreOperation },
    BackendFailure { operation: CredentialStoreOperation },
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReference => formatter.write_str("credential reference is invalid"),
            Self::InvalidServiceName => {
                formatter.write_str("credential store service name is invalid")
            }
            Self::InvalidSecret => formatter.write_str("credential secret must not be empty"),
            Self::BackendUnavailable { operation } => {
                write!(
                    formatter,
                    "credential store is unavailable during {operation}"
                )
            }
            Self::AccessDenied { operation } => {
                write!(
                    formatter,
                    "credential store access was denied during {operation}"
                )
            }
            Self::Unsupported { operation } => {
                write!(formatter, "credential store does not support {operation}")
            }
            Self::CorruptedEntry { operation } => {
                write!(
                    formatter,
                    "credential entry is malformed during {operation}"
                )
            }
            Self::AmbiguousEntry { operation } => {
                write!(
                    formatter,
                    "credential reference is ambiguous during {operation}"
                )
            }
            Self::InvalidBackendInput { operation } => {
                write!(
                    formatter,
                    "credential backend rejected input during {operation}"
                )
            }
            Self::BackendFailure { operation } => {
                write!(formatter, "credential backend failed during {operation}")
            }
        }
    }
}

impl Error for CredentialStoreError {}

/// Thread-safe, object-safe storage for provider credentials.
///
/// `replace` has create-or-overwrite semantics. `get` returns `Ok(None)` for a
/// missing entry. `delete` is idempotent and reports whether an entry existed.
pub trait CredentialStore: Send + Sync {
    /// Returns the backend identity embedded in references created by this store.
    fn backend(&self) -> CredentialStoreBackend;

    /// Creates a fresh reference owned by this store.
    fn new_reference(&self) -> CredentialReference {
        CredentialReference::new_for_backend(self.backend())
    }

    /// Reports whether this store owns `reference`.
    fn supports_reference(&self, reference: &CredentialReference) -> bool {
        reference.backend() == self.backend()
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError>;

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError>;

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError>;
}

/// Native operating-system implementation backed by keyring-rs.
///
/// keyring-rs selects Keychain Services on macOS, Windows Credential Manager on
/// Windows, and Secret Service on supported Unix desktops. Operations are
/// serialized process-wide because Windows does not guarantee ordering for
/// concurrent access to the same credential.
pub struct SystemCredentialStore {
    service: String,
}

static SYSTEM_CREDENTIAL_LOCK: Mutex<()> = Mutex::new(());

impl SystemCredentialStore {
    pub fn new(service: impl Into<String>) -> Result<Self, CredentialStoreError> {
        let service = service.into();
        validate_service_name(&service)?;
        Ok(Self { service })
    }

    #[must_use]
    pub fn image_generation() -> Self {
        Self {
            service: IMAGE_GENERATION_CREDENTIAL_SERVICE.to_owned(),
        }
    }

    #[must_use]
    pub fn model_provider() -> Self {
        Self {
            service: MODEL_PROVIDER_CREDENTIAL_SERVICE.to_owned(),
        }
    }

    fn entry(&self, reference: &CredentialReference) -> Result<Entry, CredentialStoreError> {
        Entry::new(&self.service, reference.as_str())
            .map_err(|error| classify_keyring_error(CredentialStoreOperation::Open, error))
    }
}

impl fmt::Debug for SystemCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SystemCredentialStore")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

impl CredentialStore for SystemCredentialStore {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::SystemV2
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let entry = self.entry(reference)?;
        entry
            .set_password(&secret.0)
            .map_err(|error| classify_keyring_error(CredentialStoreOperation::Replace, error))
    }

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let entry = self.entry(reference)?;
        match entry.get_password() {
            Ok(secret) => CredentialSecret::new(secret).map(Some),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(classify_keyring_error(CredentialStoreOperation::Get, error)),
        }
    }

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let entry = self.entry(reference)?;
        match entry.delete_credential() {
            Ok(()) => Ok(CredentialDeleteOutcome::Deleted),
            Err(KeyringError::NoEntry) => Ok(CredentialDeleteOutcome::NotFound),
            Err(error) => Err(classify_keyring_error(
                CredentialStoreOperation::Delete,
                error,
            )),
        }
    }
}

/// Deterministic test implementation that never touches the operating system.
#[derive(Default)]
pub struct InMemoryCredentialStore {
    credentials: Mutex<HashMap<CredentialReference, CredentialSecret>>,
}

impl fmt::Debug for InMemoryCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InMemoryCredentialStore([REDACTED])")
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::InMemoryV1
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        self.lock_credentials(CredentialStoreOperation::Replace)?
            .insert(reference.clone(), secret);
        Ok(())
    }

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        self.lock_credentials(CredentialStoreOperation::Get)?
            .get(reference)
            .map(|secret| CredentialSecret::new(secret.0.clone()))
            .transpose()
    }

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        ensure_supported_reference(self, reference)?;
        let removed = self
            .lock_credentials(CredentialStoreOperation::Delete)?
            .remove(reference);
        Ok(if removed.is_some() {
            CredentialDeleteOutcome::Deleted
        } else {
            CredentialDeleteOutcome::NotFound
        })
    }
}

impl InMemoryCredentialStore {
    fn lock_credentials(
        &self,
        operation: CredentialStoreOperation,
    ) -> Result<MutexGuard<'_, HashMap<CredentialReference, CredentialSecret>>, CredentialStoreError>
    {
        self.credentials
            .lock()
            .map_err(|_| CredentialStoreError::BackendFailure { operation })
    }
}

fn parse_reference(value: &str) -> Result<(CredentialStoreBackend, &str), CredentialStoreError> {
    let Some(suffix) = value
        .strip_prefix(OPAQUE_REFERENCE_PREFIX)
        .or_else(|| value.strip_prefix(LEGACY_IMAGE_REFERENCE_PREFIX))
    else {
        return Err(CredentialStoreError::InvalidReference);
    };
    let (backend, opaque_id) = match suffix.split_once('/') {
        Some((backend, opaque_id)) => (
            CredentialStoreBackend::parse(backend).ok_or(CredentialStoreError::InvalidReference)?,
            opaque_id,
        ),
        None => (CredentialStoreBackend::LegacySystemV1, suffix),
    };
    if opaque_id.len() != OPAQUE_REFERENCE_UUID_BYTES
        || !opaque_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CredentialStoreError::InvalidReference);
    }
    Ok((backend, opaque_id))
}

fn ensure_supported_reference(
    store: &dyn CredentialStore,
    reference: &CredentialReference,
) -> Result<(), CredentialStoreError> {
    if store.supports_reference(reference) {
        Ok(())
    } else {
        Err(CredentialStoreError::InvalidReference)
    }
}

fn validate_service_name(value: &str) -> Result<(), CredentialStoreError> {
    if value.is_empty()
        || value.len() > MAX_CREDENTIAL_SERVICE_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CredentialStoreError::InvalidServiceName);
    }
    Ok(())
}

fn lock_system_credentials() -> MutexGuard<'static, ()> {
    SYSTEM_CREDENTIAL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn classify_keyring_error(
    operation: CredentialStoreOperation,
    error: KeyringError,
) -> CredentialStoreError {
    match error {
        KeyringError::NoDefaultStore => CredentialStoreError::BackendUnavailable { operation },
        KeyringError::NoStorageAccess(_) => CredentialStoreError::AccessDenied { operation },
        KeyringError::NotSupportedByStore(_) => CredentialStoreError::Unsupported { operation },
        KeyringError::BadEncoding(_)
        | KeyringError::BadDataFormat(_, _)
        | KeyringError::BadStoreFormat(_) => CredentialStoreError::CorruptedEntry { operation },
        KeyringError::Ambiguous(_) => CredentialStoreError::AmbiguousEntry { operation },
        KeyringError::TooLong(_, _) | KeyringError::Invalid(_, _) => {
            CredentialStoreError::InvalidBackendInput { operation }
        }
        KeyringError::NoEntry | KeyringError::PlatformFailure(_) => {
            CredentialStoreError::BackendFailure { operation }
        }
        _ => CredentialStoreError::BackendFailure { operation },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn secret_text(secret: &CredentialSecret) -> String {
        secret.with_secret_bytes(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn opaque_references_are_unique_and_valid() {
        let first = CredentialReference::new_opaque();
        let second = CredentialReference::new_opaque();

        assert_ne!(first, second);
        assert!(first.as_str().starts_with(OPAQUE_REFERENCE_PREFIX));
        assert!(CredentialReference::parse(first.as_str()).is_ok());
    }

    #[test]
    fn references_preserve_backend_ownership_and_parse_legacy_values() {
        for backend in [
            CredentialStoreBackend::SystemV2,
            CredentialStoreBackend::MacKeychainV2,
            CredentialStoreBackend::DevelopmentFileV1,
            CredentialStoreBackend::InMemoryV1,
        ] {
            let reference = CredentialReference::new_for_backend(backend);
            let parsed = CredentialReference::parse(reference.as_str()).unwrap();
            assert_eq!(parsed.backend(), backend);
            assert_eq!(parsed, reference);
        }

        let legacy =
            CredentialReference::parse("image-generation/api-key/0123456789abcdef0123456789abcdef")
                .unwrap();
        assert_eq!(legacy.backend(), CredentialStoreBackend::LegacySystemV1);
        assert!(!InMemoryCredentialStore::default().supports_reference(&legacy));
    }

    #[test]
    fn stable_backend_owned_reference_is_deterministic_without_changing_random_references() {
        const STABLE_ID: &str = "9e5289f6297448d1b7e8895e5eb0d033";
        let first = CredentialReference::from_stable_opaque_id(
            CredentialStoreBackend::InMemoryV1,
            STABLE_ID,
        )
        .unwrap();
        let second = CredentialReference::from_stable_opaque_id(
            CredentialStoreBackend::InMemoryV1,
            STABLE_ID,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(CredentialReference::parse(first.as_str()).unwrap(), first);
        assert_ne!(
            CredentialReference::new_for_backend(CredentialStoreBackend::InMemoryV1),
            second
        );
        assert!(CredentialReference::from_stable_opaque_id(
            CredentialStoreBackend::LegacySystemV1,
            STABLE_ID
        )
        .is_err());
        assert!(CredentialReference::from_stable_opaque_id(
            CredentialStoreBackend::InMemoryV1,
            "NOT-A-CANONICAL-STABLE-ID"
        )
        .is_err());
    }

    #[test]
    fn invalid_references_are_rejected_without_echoing_input() {
        let invalid = "secret-looking\nreference";
        let error = CredentialReference::parse(invalid).unwrap_err();

        assert_eq!(error, CredentialStoreError::InvalidReference);
        assert!(!error.to_string().contains(invalid));
    }

    #[test]
    fn credential_secret_debug_output_is_always_redacted() {
        let value = "super-secret-api-key";
        let secret = CredentialSecret::new(value).unwrap();
        let debug = format!("{secret:?}");

        assert_eq!(debug, "CredentialSecret([REDACTED])");
        assert!(!debug.contains(value));
    }

    #[test]
    fn credential_reference_debug_output_is_always_redacted() {
        let reference = CredentialReference::new_opaque();
        let persisted_reference = reference.as_str().to_string();
        let debug = format!("{reference:?}");

        assert_eq!(debug, "CredentialReference([REDACTED])");
        assert!(!debug.contains(&persisted_reference));
    }

    #[test]
    fn empty_credential_secrets_are_rejected() {
        assert_eq!(
            CredentialSecret::new("").unwrap_err(),
            CredentialStoreError::InvalidSecret
        );
    }

    #[test]
    fn in_memory_store_has_explicit_replace_get_and_delete_semantics() {
        let store = InMemoryCredentialStore::default();
        let reference = CredentialReference::new_opaque();

        assert!(store.get(&reference).unwrap().is_none());

        store
            .replace(&reference, CredentialSecret::new("first-secret").unwrap())
            .unwrap();
        assert_eq!(
            secret_text(&store.get(&reference).unwrap().unwrap()),
            "first-secret"
        );

        store
            .replace(
                &reference,
                CredentialSecret::new("replacement-secret").unwrap(),
            )
            .unwrap();
        assert_eq!(
            secret_text(&store.get(&reference).unwrap().unwrap()),
            "replacement-secret"
        );

        assert_eq!(
            store.delete(&reference).unwrap(),
            CredentialDeleteOutcome::Deleted
        );
        assert_eq!(
            store.delete(&reference).unwrap(),
            CredentialDeleteOutcome::NotFound
        );
        assert!(store.get(&reference).unwrap().is_none());
    }

    #[test]
    fn credential_store_is_object_safe_and_thread_safe() {
        let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::default());
        let reference = CredentialReference::new_opaque();

        let writer_store = Arc::clone(&store);
        let writer_reference = reference.clone();
        std::thread::spawn(move || {
            writer_store
                .replace(
                    &writer_reference,
                    CredentialSecret::new("thread-secret").unwrap(),
                )
                .unwrap();
        })
        .join()
        .unwrap();

        assert_eq!(
            secret_text(&store.get(&reference).unwrap().unwrap()),
            "thread-secret"
        );
    }

    #[test]
    fn store_debug_output_does_not_contain_credentials() {
        let store = InMemoryCredentialStore::default();
        let reference = CredentialReference::new_opaque();
        store
            .replace(
                &reference,
                CredentialSecret::new("never-print-this").unwrap(),
            )
            .unwrap();

        let debug = format!("{store:?}");
        assert_eq!(debug, "InMemoryCredentialStore([REDACTED])");
        assert!(!debug.contains("never-print-this"));
    }
}
