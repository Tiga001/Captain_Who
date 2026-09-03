//! Non-interactive macOS Keychain credential storage.
//!
//! Every operation disables Keychain UI for its complete lifetime. Reads and
//! deletes additionally set `kSecUseAuthenticationUI=Skip`. If an item needs
//! user interaction, the operation fails with a redacted error instead of
//! presenting a password dialog from the background helper.

use std::fmt;

use security_framework::{
    base::Error as SecurityFrameworkError,
    item::{ItemClass, ItemSearchOptions, SearchResult},
    os::macos::keychain::SecKeychain,
};
use security_framework_sys::base::{errSecAuthFailed, errSecDuplicateItem, errSecItemNotFound};

use super::{
    ensure_supported_reference, lock_system_credentials, validate_service_name,
    CredentialDeleteOutcome, CredentialReference, CredentialSecret, CredentialStore,
    CredentialStoreBackend, CredentialStoreError, CredentialStoreOperation,
    IMAGE_GENERATION_CREDENTIAL_SERVICE, MODEL_PROVIDER_CREDENTIAL_SERVICE,
};

// Security.framework declares this value in SecBase.h, but the sys crate does
// not currently expose it.
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25_308;

/// Keychain adapter for signed macOS distributions.
///
/// This adapter uses a new service namespace and backend-tagged references, so
/// it never queries an item created by the legacy ad-hoc-signed helper.
pub struct NonInteractiveMacCredentialStore {
    service: String,
}

impl NonInteractiveMacCredentialStore {
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

    fn search_options(
        &self,
        reference: &CredentialReference,
        load_data: bool,
    ) -> ItemSearchOptions {
        let mut options = ItemSearchOptions::new();
        options
            .class(ItemClass::generic_password())
            .service(&self.service)
            .account(reference.as_str())
            .limit(2)
            .load_data(load_data)
            .skip_authenticated_items(true);
        options
    }
}

impl fmt::Debug for NonInteractiveMacCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NonInteractiveMacCredentialStore")
            .field("service", &self.service)
            .finish_non_exhaustive()
    }
}

impl CredentialStore for NonInteractiveMacCredentialStore {
    fn backend(&self) -> CredentialStoreBackend {
        CredentialStoreBackend::MacKeychainV2
    }

    fn replace(
        &self,
        reference: &CredentialReference,
        secret: CredentialSecret,
    ) -> Result<(), CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Replace;
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let _interaction_lock = SecKeychain::disable_user_interaction()
            .map_err(|error| classify_security_error(OPERATION, error))?;
        let keychain = SecKeychain::default()
            .map_err(|error| classify_security_error(CredentialStoreOperation::Open, error))?;
        secret.with_secret_bytes(|bytes| {
            keychain
                .set_generic_password(&self.service, reference.as_str(), bytes)
                .map_err(|error| classify_security_error(OPERATION, error))
        })
    }

    fn get(
        &self,
        reference: &CredentialReference,
    ) -> Result<Option<CredentialSecret>, CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Get;
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let _interaction_lock = SecKeychain::disable_user_interaction()
            .map_err(|error| classify_security_error(OPERATION, error))?;
        match self.search_options(reference, true).search() {
            Ok(results) => {
                let mut results = results.into_iter();
                let Some(SearchResult::Data(bytes)) = results.next() else {
                    return Ok(None);
                };
                if results.next().is_some() {
                    return Err(CredentialStoreError::AmbiguousEntry {
                        operation: OPERATION,
                    });
                }
                let secret =
                    String::from_utf8(bytes).map_err(|_| CredentialStoreError::CorruptedEntry {
                        operation: OPERATION,
                    })?;
                CredentialSecret::new(secret).map(Some)
            }
            Err(error) if error.code() == errSecItemNotFound => Ok(None),
            Err(error) => Err(classify_security_error(OPERATION, error)),
        }
    }

    fn delete(
        &self,
        reference: &CredentialReference,
    ) -> Result<CredentialDeleteOutcome, CredentialStoreError> {
        const OPERATION: CredentialStoreOperation = CredentialStoreOperation::Delete;
        ensure_supported_reference(self, reference)?;
        let _guard = lock_system_credentials();
        let _interaction_lock = SecKeychain::disable_user_interaction()
            .map_err(|error| classify_security_error(OPERATION, error))?;
        match self.search_options(reference, false).delete() {
            Ok(()) => Ok(CredentialDeleteOutcome::Deleted),
            Err(error) if error.code() == errSecItemNotFound => {
                Ok(CredentialDeleteOutcome::NotFound)
            }
            Err(error) => Err(classify_security_error(OPERATION, error)),
        }
    }
}

fn classify_security_error(
    operation: CredentialStoreOperation,
    error: SecurityFrameworkError,
) -> CredentialStoreError {
    let code = error.code();
    if code == errSecAuthFailed || code == ERR_SEC_INTERACTION_NOT_ALLOWED {
        CredentialStoreError::AccessDenied { operation }
    } else if code == errSecDuplicateItem {
        CredentialStoreError::AmbiguousEntry { operation }
    } else {
        CredentialStoreError::BackendFailure { operation }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_uses_v2_namespace_and_rejects_legacy_references_before_keychain_access() {
        let store = NonInteractiveMacCredentialStore::image_generation();
        let legacy =
            CredentialReference::parse("image-generation/api-key/0123456789abcdef0123456789abcdef")
                .unwrap();

        assert_eq!(store.backend(), CredentialStoreBackend::MacKeychainV2);
        assert!(store.service.ends_with(".v2"));
        assert_eq!(
            store.get(&legacy).unwrap_err(),
            CredentialStoreError::InvalidReference
        );
    }
}
