use super::credential_store::CredentialSecret;
use super::types::{
    ImageGenerationError, ImageGenerationProviderProfile, ImageGenerationRequest,
    ImageGenerationResult, PreparedImageGenerationRequest,
};
use futures_util::future::BoxFuture;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Provider-neutral image-generation contract.
///
/// Implementations receive only a normalized request and a separately borrowed credential. Raw
/// vendor fields, endpoints, model identifiers, and credentials are never accepted from the
/// model-facing request. Dropping the returned future is the cancellation boundary; adapters must
/// not spawn detached network work.
pub trait ImageGenerationProvider: Send + Sync {
    fn profile(&self) -> &ImageGenerationProviderProfile;

    fn prepare(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<PreparedImageGenerationRequest, ImageGenerationError>;

    fn execute<'a>(
        &'a self,
        request: &'a PreparedImageGenerationRequest,
        credential: &'a CredentialSecret,
    ) -> BoxFuture<'a, Result<ImageGenerationResult, ImageGenerationError>>;
}

/// Constructs provider instances for one stable adapter id.
///
/// Configuration and agent code depend on this provider-neutral factory boundary rather than a
/// central vendor `match`. Adding an adapter therefore consists of registering another factory;
/// it does not require teaching the execution pipeline how that vendor creates HTTP clients.
pub trait ImageGenerationProviderFactory: Send + Sync {
    fn adapter_id(&self) -> super::types::ImageGenerationAdapterId;

    fn create(
        &self,
        profile: ImageGenerationProviderProfile,
    ) -> Result<Arc<dyn ImageGenerationProvider>, ImageGenerationError>;
}

/// Registry of available adapter factories.
///
/// This is intentionally separate from [`ImageGenerationProviderRegistry`]: adapter factories
/// are application capabilities, while provider instances are immutable snapshots of configured
/// profiles for a particular run.
#[derive(Default)]
pub struct ImageGenerationAdapterRegistry {
    factories: BTreeMap<String, Arc<dyn ImageGenerationProviderFactory>>,
}

impl ImageGenerationAdapterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        factory: Arc<dyn ImageGenerationProviderFactory>,
    ) -> Result<(), ImageGenerationError> {
        let adapter_id = factory.adapter_id().as_str().to_string();
        if self.factories.contains_key(&adapter_id) {
            return Err(ImageGenerationError::invalid_configuration(
                "an image-generation factory with this adapter id is already registered",
            ));
        }
        self.factories.insert(adapter_id, factory);
        Ok(())
    }

    pub fn create(
        &self,
        profile: ImageGenerationProviderProfile,
    ) -> Result<Arc<dyn ImageGenerationProvider>, ImageGenerationError> {
        profile.validate()?;
        let factory = self
            .factories
            .get(profile.adapter_id.as_str())
            .ok_or_else(|| {
                ImageGenerationError::invalid_configuration(
                    "the configured image-generation adapter is not registered",
                )
            })?;
        let expected_profile = profile.clone();
        let provider = factory.create(profile)?;
        if provider.profile() != &expected_profile {
            return Err(ImageGenerationError::invalid_configuration(
                "image-generation factory returned a provider for a different frozen profile",
            ));
        }
        Ok(provider)
    }

    #[must_use]
    pub fn contains(&self, adapter_id: super::types::ImageGenerationAdapterId) -> bool {
        self.factories.contains_key(adapter_id.as_str())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl fmt::Debug for ImageGenerationAdapterRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationAdapterRegistry")
            .field("adapter_ids", &self.factories.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Registry of active provider profiles keyed by stable profile id.
///
/// Persisted enablement and credential resolution remain host responsibilities. Only validated,
/// currently active providers should be registered for an agent run.
#[derive(Default)]
pub struct ImageGenerationProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ImageGenerationProvider>>,
}

impl ImageGenerationProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        provider: Arc<dyn ImageGenerationProvider>,
    ) -> Result<(), ImageGenerationError> {
        provider.profile().validate()?;
        let id = provider.profile().id.clone();
        if self.providers.contains_key(&id) {
            return Err(ImageGenerationError::invalid_configuration(
                "an image-generation provider with this profile id is already registered",
            ));
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    #[must_use]
    pub fn get(&self, profile_id: &str) -> Option<Arc<dyn ImageGenerationProvider>> {
        self.providers.get(profile_id).cloned()
    }

    #[must_use]
    pub fn contains(&self, profile_id: &str) -> bool {
        self.providers.contains_key(profile_id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn profiles(&self) -> impl Iterator<Item = &ImageGenerationProviderProfile> {
        self.providers.values().map(|provider| provider.profile())
    }
}

impl fmt::Debug for ImageGenerationProviderRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageGenerationProviderRegistry")
            .field("profile_ids", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_generation::types::{
        ImageGenerationAdapterId, ImageGenerationCapabilities, ImageGenerationDefaults,
    };

    struct TestProvider {
        profile: ImageGenerationProviderProfile,
    }

    struct TestFactory;

    struct MismatchedFactory;

    impl ImageGenerationProviderFactory for TestFactory {
        fn adapter_id(&self) -> ImageGenerationAdapterId {
            ImageGenerationAdapterId::SmartMlSeedream
        }

        fn create(
            &self,
            profile: ImageGenerationProviderProfile,
        ) -> Result<Arc<dyn ImageGenerationProvider>, ImageGenerationError> {
            Ok(Arc::new(TestProvider { profile }))
        }
    }

    impl ImageGenerationProviderFactory for MismatchedFactory {
        fn adapter_id(&self) -> ImageGenerationAdapterId {
            ImageGenerationAdapterId::SmartMlSeedream
        }

        fn create(
            &self,
            mut profile: ImageGenerationProviderProfile,
        ) -> Result<Arc<dyn ImageGenerationProvider>, ImageGenerationError> {
            profile.model_id = "different-model".to_string();
            Ok(Arc::new(TestProvider { profile }))
        }
    }

    impl ImageGenerationProvider for TestProvider {
        fn profile(&self) -> &ImageGenerationProviderProfile {
            &self.profile
        }

        fn prepare(
            &self,
            request: ImageGenerationRequest,
        ) -> Result<PreparedImageGenerationRequest, ImageGenerationError> {
            let request = request.normalize(&self.profile)?;
            Ok(PreparedImageGenerationRequest::new(&self.profile, request))
        }

        fn execute<'a>(
            &'a self,
            _request: &'a PreparedImageGenerationRequest,
            _credential: &'a CredentialSecret,
        ) -> BoxFuture<'a, Result<ImageGenerationResult, ImageGenerationError>> {
            Box::pin(async move { Ok(ImageGenerationResult::ready(&self.profile)) })
        }
    }

    fn provider(id: &str) -> Arc<dyn ImageGenerationProvider> {
        Arc::new(TestProvider {
            profile: ImageGenerationProviderProfile::new(
                id,
                ImageGenerationAdapterId::SmartMlSeedream,
                "https://images.example/v1/images/generations",
                "seedream-model",
                1,
                ImageGenerationCapabilities::smartml_seedream(false),
                ImageGenerationDefaults::default(),
            )
            .unwrap(),
        })
    }

    #[test]
    fn registry_rejects_duplicate_profile_ids() {
        let mut registry = ImageGenerationProviderRegistry::new();
        registry.register(provider("default")).unwrap();
        let error = registry.register(provider("default")).unwrap_err();

        assert_eq!(
            error.code,
            crate::image_generation::types::ImageGenerationErrorCode::InvalidConfiguration
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_debug_contains_only_profile_ids() {
        let mut registry = ImageGenerationProviderRegistry::new();
        registry.register(provider("default")).unwrap();
        let debug = format!("{registry:?}");

        assert!(debug.contains("default"));
        assert!(!debug.contains("seedream-model"));
        assert!(!debug.contains("images.example"));
    }

    #[test]
    fn adapter_registry_builds_profiles_without_a_vendor_switch() {
        let profile = provider("default").profile().clone();
        let mut registry = ImageGenerationAdapterRegistry::new();
        registry.register(Arc::new(TestFactory)).unwrap();

        let created = registry.create(profile.clone()).unwrap();

        assert_eq!(created.profile(), &profile);
        assert!(registry.contains(ImageGenerationAdapterId::SmartMlSeedream));
        assert_eq!(registry.len(), 1);
        assert!(registry.register(Arc::new(TestFactory)).is_err());
    }

    #[test]
    fn adapter_registry_rejects_a_provider_for_a_different_profile() {
        let profile = provider("default").profile().clone();
        let mut registry = ImageGenerationAdapterRegistry::new();
        registry.register(Arc::new(MismatchedFactory)).unwrap();

        let error = match registry.create(profile) {
            Ok(_) => panic!("mismatched provider profile must be rejected"),
            Err(error) => error,
        };

        assert_eq!(
            error.code,
            crate::image_generation::types::ImageGenerationErrorCode::InvalidConfiguration
        );
    }
}
