//! Safe transfer and durable publication of provider-generated image artifacts.
//!
//! Provider output URLs are untrusted, ephemeral capabilities. This module consumes them without
//! forwarding provider credentials, validates every network hop against an SSRF policy, fully
//! decodes the downloaded image, and publishes it into an application-managed immutable store.

use super::types::ImageGenerationUrlOutput;
use crate::durable_fs::{atomic_rename_noreplace, sync_directory};
use crate::AgentCancellationToken;
use futures_util::future::{join, join_all, BoxFuture};
use futures_util::StreamExt;
use image::{ImageFormat, ImageReader, Limits};
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, LOCATION,
};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use uuid::Uuid;

pub const DEFAULT_IMAGE_ARTIFACT_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const DEFAULT_IMAGE_ARTIFACT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_IMAGE_ARTIFACT_DNS_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_IMAGE_ARTIFACT_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_IMAGE_ARTIFACT_MAX_REDIRECTS: usize = 3;
pub const MAX_IMAGE_ARTIFACT_DIMENSION: u32 = 16_384;
pub const MAX_IMAGE_ARTIFACT_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_IMAGE_ARTIFACT_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
const MAX_IMAGE_ARTIFACT_URL_BYTES: usize = 16 * 1024;
const MANAGED_ARTIFACT_OBJECTS_DIRECTORY: &str = "objects";
const MAX_PUBLIC_DNS_RESPONSE_BYTES: usize = 64 * 1024;

const ALIDNS_ADDRESSES: &[&str] = &["223.5.5.5", "223.6.6.6"];
const GOOGLE_DNS_ADDRESSES: &[&str] = &["8.8.8.8", "8.8.4.4"];
const PUBLIC_DNS_RESOLVERS: &[PublicDnsResolver] = &[
    PublicDnsResolver {
        host: "dns.alidns.com",
        path: "/resolve",
        addresses: ALIDNS_ADDRESSES,
    },
    PublicDnsResolver {
        host: "dns.google",
        path: "/resolve",
        addresses: GOOGLE_DNS_ADDRESSES,
    },
];

#[derive(Debug, Clone, Copy)]
struct PublicDnsResolver {
    host: &'static str,
    path: &'static str,
    addresses: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageArtifactNetworkPolicy {
    PublicHttpsOnly,
    /// Test-only escape hatch. HTTP and non-public addresses remain limited to loopback.
    AllowLoopbackHttpForTests,
}

/// Compile-time hostname rule minted by a trusted Provider Adapter.
///
/// These rules are not model input, user configuration, or provider response data. They grant
/// only the narrow ability to traverse a local Fake-IP TUN/proxy that represents an HTTPS origin
/// with an RFC 2544 benchmarking address. URL, TLS/SNI, redirect, DNS pinning, and remote-address
/// checks remain mandatory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageArtifactHostRule {
    ExactHttpsOrigin { host: &'static str, port: u16 },
}

impl ImageArtifactHostRule {
    fn matches(self, host: &str, port: u16) -> bool {
        match self {
            Self::ExactHttpsOrigin {
                host: expected_host,
                port: expected_port,
            } => expected_host.eq_ignore_ascii_case(host) && expected_port == port,
        }
    }
}

/// Provider-owned network capability for downloading one generated Artifact.
///
/// The default has no direct Fake-IP exceptions. An adapter may add reviewed exact hosts, and a
/// validated provider profile may freeze its user-configured endpoint host. Other Fake-IP hosts
/// are resolved independently through pinned public DNS and connected by their verified public
/// addresses; provider response data and model input cannot mint a direct-tunnel exception.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageArtifactTransferPolicy {
    trusted_fake_ip_https_hosts: &'static [ImageArtifactHostRule],
    configured_endpoint_https_origin: Option<(Arc<str>, u16)>,
}

impl ImageArtifactTransferPolicy {
    #[must_use]
    pub const fn public_https_only() -> Self {
        Self {
            trusted_fake_ip_https_hosts: &[],
            configured_endpoint_https_origin: None,
        }
    }

    #[must_use]
    pub const fn with_trusted_fake_ip_https_hosts(hosts: &'static [ImageArtifactHostRule]) -> Self {
        Self {
            trusted_fake_ip_https_hosts: hosts,
            configured_endpoint_https_origin: None,
        }
    }

    #[must_use]
    pub fn with_configured_endpoint_https_origin(mut self, host: &str, port: u16) -> Self {
        if fake_ip_eligible_dns_hostname(host) && port != 0 {
            self.configured_endpoint_https_origin =
                Some((Arc::<str>::from(host.to_ascii_lowercase()), port));
        }
        self
    }

    pub(crate) fn permits_fake_ip_for_origin(&self, host: &str, port: u16) -> bool {
        literal_ip(host).is_none()
            && (self.configured_endpoint_https_origin.as_ref().is_some_and(
                |(expected_host, expected_port)| {
                    expected_host.eq_ignore_ascii_case(host) && *expected_port == port
                },
            ) || self
                .trusted_fake_ip_https_hosts
                .iter()
                .any(|rule| rule.matches(host, port)))
    }

    fn permits_configured_https_origin(&self, host: &str, port: u16) -> bool {
        self.configured_endpoint_https_origin.as_ref().is_some_and(
            |(expected_host, expected_port)| {
                expected_host.eq_ignore_ascii_case(host) && *expected_port == port
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageArtifactStoreConfig {
    pub network_policy: ImageArtifactNetworkPolicy,
    pub connect_timeout: Duration,
    pub dns_timeout: Duration,
    pub download_timeout: Duration,
    pub max_download_bytes: usize,
    pub max_redirects: usize,
}

impl Default for ImageArtifactStoreConfig {
    fn default() -> Self {
        Self {
            network_policy: ImageArtifactNetworkPolicy::PublicHttpsOnly,
            connect_timeout: DEFAULT_IMAGE_ARTIFACT_CONNECT_TIMEOUT,
            dns_timeout: DEFAULT_IMAGE_ARTIFACT_DNS_TIMEOUT,
            download_timeout: DEFAULT_IMAGE_ARTIFACT_DOWNLOAD_TIMEOUT,
            max_download_bytes: DEFAULT_IMAGE_ARTIFACT_MAX_BYTES,
            max_redirects: DEFAULT_IMAGE_ARTIFACT_MAX_REDIRECTS,
        }
    }
}

impl ImageArtifactStoreConfig {
    fn validate(self) -> Result<Self, ImageArtifactError> {
        if self.connect_timeout.is_zero()
            || self.dns_timeout.is_zero()
            || self.download_timeout.is_zero()
            || self.connect_timeout > self.download_timeout
            || self.dns_timeout > self.download_timeout
            || self.download_timeout > Duration::from_secs(10 * 60)
        {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidConfiguration,
                "image Artifact transfer timeouts are invalid",
                false,
            ));
        }
        if self.max_download_bytes == 0 || self.max_download_bytes > 128 * 1024 * 1024 {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidConfiguration,
                "image Artifact byte limit is invalid",
                false,
            ));
        }
        if self.max_redirects > 5 {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidConfiguration,
                "image Artifact redirect limit is invalid",
                false,
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageArtifactFormat {
    Png,
    Jpeg,
    Webp,
}

impl ImageArtifactFormat {
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    const fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenerationArtifactCandidate {
    pub artifact_id: String,
    pub storage_relative_path: String,
    pub format: ImageArtifactFormat,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub sha256: String,
}

impl ImageGenerationArtifactCandidate {
    #[must_use]
    pub fn artifact_uri(&self) -> String {
        format!("image-artifact://sha256/{}", self.sha256)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageArtifactPublicationStatus {
    Created,
    AlreadyPresent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedImageArtifact {
    pub candidate: ImageGenerationArtifactCandidate,
    pub absolute_path: PathBuf,
    pub status: ImageArtifactPublicationStatus,
}

impl PublishedImageArtifact {
    /// Reopens and fully revalidates the immutable published file before its bytes cross another
    /// trust boundary, such as the transient model-vision delivery path.
    ///
    /// The caller must still decide whether the receiving model is allowed to accept image input.
    /// This method deliberately performs no encoding and exposes no provider URL.
    pub(crate) fn read_verified(&self, max_bytes: usize) -> Result<Vec<u8>, ImageArtifactError> {
        if !self.absolute_path.is_absolute() {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::Conflict,
                "published image Artifact path is not absolute",
                false,
            ));
        }
        read_file_matching_candidate(&self.absolute_path, &self.candidate, max_bytes)
    }
}

/// Fully revalidated content read from the private immutable Artifact store.
///
/// The managed path remains intentionally absent: callers receive only the same presentation-safe
/// identity that was persisted in the Agent result plus the verified image bytes.
pub struct ManagedImageArtifactContent {
    pub candidate: ImageGenerationArtifactCandidate,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for ManagedImageArtifactContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedImageArtifactContent")
            .field("candidate", &self.candidate)
            .field("byte_length", &self.bytes.len())
            .finish()
    }
}

/// A validated but unpublished Artifact. Dropping it removes its private staging file.
pub struct PreparedImageArtifact {
    candidate: ImageGenerationArtifactCandidate,
    staging_path: PathBuf,
    target_path: PathBuf,
    published: bool,
}

impl PreparedImageArtifact {
    #[must_use]
    pub fn candidate(&self) -> &ImageGenerationArtifactCandidate {
        &self.candidate
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        candidate: ImageGenerationArtifactCandidate,
        staging_path: PathBuf,
        target_path: PathBuf,
    ) -> Self {
        Self {
            candidate,
            staging_path,
            target_path,
            published: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_published_for_test(&mut self) {
        self.published = true;
    }
}

impl fmt::Debug for PreparedImageArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedImageArtifact")
            .field("candidate", &self.candidate)
            .field("published", &self.published)
            .finish_non_exhaustive()
    }
}

impl Drop for PreparedImageArtifact {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.staging_path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageArtifactErrorCode {
    InvalidConfiguration,
    UnsafeUrl,
    DnsRejected,
    RedirectRejected,
    TransportFailed,
    DownloadTimedOut,
    HttpRejected,
    ResponseTooLarge,
    UnsupportedMediaType,
    InvalidImage,
    Io,
    Conflict,
    Cancelled,
    CommitIndeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageArtifactError {
    pub code: ImageArtifactErrorCode,
    pub message: String,
    pub retryable: bool,
    pub http_status: Option<u16>,
    pub commit_may_have_succeeded: bool,
}

impl ImageArtifactError {
    #[must_use]
    pub fn new(code: ImageArtifactErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            http_status: None,
            commit_may_have_succeeded: false,
        }
    }

    fn cancelled() -> Self {
        Self::new(
            ImageArtifactErrorCode::Cancelled,
            "image Artifact transfer was cancelled",
            false,
        )
    }

    fn commit_indeterminate() -> Self {
        Self {
            code: ImageArtifactErrorCode::CommitIndeterminate,
            message: "image Artifact publication outcome is indeterminate".to_string(),
            retryable: false,
            http_status: None,
            commit_may_have_succeeded: true,
        }
    }

    #[must_use]
    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }
}

impl fmt::Display for ImageArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImageArtifactError {}

pub trait ImageGenerationArtifactStore: Send + Sync {
    fn stage<'a>(
        &'a self,
        source: &'a ImageGenerationUrlOutput,
        transfer_policy: ImageArtifactTransferPolicy,
        cancellation: &'a AgentCancellationToken,
    ) -> BoxFuture<'a, Result<PreparedImageArtifact, ImageArtifactError>>;

    fn publish(
        &self,
        prepared: PreparedImageArtifact,
        cancellation: &AgentCancellationToken,
    ) -> Result<PublishedImageArtifact, ImageArtifactError>;

    fn inspect(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError>;

    /// Reconciles an uncertain atomic publication and confirms its directory entry is durable.
    /// Implementations must not return `Some` until both content identity and parent-directory
    /// persistence have been verified.
    fn confirm_published(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError>;
}

#[derive(Clone)]
pub struct ManagedImageGenerationArtifactStore {
    objects_root: Arc<PathBuf>,
    config: ImageArtifactStoreConfig,
    validation_workers: Arc<Semaphore>,
}

impl ManagedImageGenerationArtifactStore {
    pub fn new(
        root: impl AsRef<Path>,
        config: ImageArtifactStoreConfig,
    ) -> Result<Self, ImageArtifactError> {
        let config = config.validate()?;
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(io_error)?;
        reject_symlink_or_non_directory(root)?;
        set_private_directory_permissions(root)?;
        let root = fs::canonicalize(root).map_err(io_error)?;
        let objects_root = root.join(MANAGED_ARTIFACT_OBJECTS_DIRECTORY);
        fs::create_dir_all(&objects_root).map_err(io_error)?;
        reject_symlink_or_non_directory(&objects_root)?;
        set_private_directory_permissions(&objects_root)?;
        sync_directory(&root).map_err(io_error)?;
        Ok(Self {
            objects_root: Arc::new(objects_root),
            config,
            // Full image decoding is deliberately kept off Tokio workers and bounded separately
            // from the runtime's global blocking pool.
            validation_workers: Arc::new(Semaphore::new(2)),
        })
    }

    #[must_use]
    pub fn config(&self) -> ImageArtifactStoreConfig {
        self.config
    }

    /// Reads an immutable Artifact after revalidating its frozen identity and image structure.
    ///
    /// Reads share the bounded image-validation pool with publication so a Renderer cannot create
    /// unbounded parallel decoder work by restoring a conversation containing many images.
    pub async fn read_published(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<Option<ManagedImageArtifactContent>, ImageArtifactError> {
        let target = self.validate_candidate_path(candidate)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(ImageArtifactError::new(
                    ImageArtifactErrorCode::Conflict,
                    "image Artifact store entry is not an immutable regular file",
                    false,
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }

        let validation_worker = Arc::clone(&self.validation_workers)
            .acquire_owned()
            .await
            .map_err(|_| {
                ImageArtifactError::new(
                    ImageArtifactErrorCode::InvalidConfiguration,
                    "image Artifact validation pool is unavailable",
                    true,
                )
            })?;
        let candidate = candidate.clone();
        let max_download_bytes = self.config.max_download_bytes;
        tokio::task::spawn_blocking(move || {
            let _validation_worker = validation_worker;
            let bytes = read_file_matching_candidate(&target, &candidate, max_download_bytes)?;
            Ok(Some(ManagedImageArtifactContent { candidate, bytes }))
        })
        .await
        .map_err(|_| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::Io,
                "image Artifact validation worker stopped unexpectedly",
                true,
            )
        })?
    }

    async fn stage_inner(
        &self,
        source: &ImageGenerationUrlOutput,
        transfer_policy: ImageArtifactTransferPolicy,
        cancellation: &AgentCancellationToken,
    ) -> Result<PreparedImageArtifact, ImageArtifactError> {
        let mut staging = StagingArtifactFile::new(&self.objects_root)?;
        let mut staging_writer =
            tokio::fs::File::from_std(staging.file.try_clone().map_err(io_error)?);
        let content_type = self
            .download(
                source.expose_url_for_download(),
                transfer_policy,
                &mut staging_writer,
                cancellation,
            )
            .await?;
        staging_writer.flush().await.map_err(io_error)?;
        staging_writer.sync_all().await.map_err(io_error)?;
        drop(staging_writer);
        let staging_path = staging.path().to_path_buf();
        let validation_worker = tokio::select! {
            _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
            worker = Arc::clone(&self.validation_workers).acquire_owned() => {
                worker.map_err(|_| ImageArtifactError::new(
                    ImageArtifactErrorCode::InvalidConfiguration,
                    "image Artifact validation pool is unavailable",
                    true,
                ))?
            }
        };
        let validation_path = staging_path.clone();
        let max_download_bytes = self.config.max_download_bytes;
        let metadata = tokio::task::spawn_blocking(move || {
            let _validation_worker = validation_worker;
            validate_staged_image(
                &validation_path,
                content_type.as_deref(),
                max_download_bytes,
                None,
            )
        })
        .await
        .map_err(|_| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::Io,
                "image Artifact validation worker stopped unexpectedly",
                true,
            )
        })??;
        cancellation_check(cancellation)?;
        let target_name = format!("{}.{}", metadata.sha256, metadata.format.extension());
        let target_path = self.objects_root.join(&target_name);
        let candidate = ImageGenerationArtifactCandidate {
            artifact_id: format!("sha256:{}", metadata.sha256),
            storage_relative_path: format!("{MANAGED_ARTIFACT_OBJECTS_DIRECTORY}/{target_name}"),
            format: metadata.format,
            media_type: metadata.format.media_type().to_string(),
            width: metadata.width,
            height: metadata.height,
            size_bytes: metadata.size_bytes,
            sha256: metadata.sha256,
        };
        staging.disarm();
        Ok(PreparedImageArtifact {
            candidate,
            staging_path,
            target_path,
            published: false,
        })
    }

    async fn download(
        &self,
        initial_url: &str,
        transfer_policy: ImageArtifactTransferPolicy,
        staging: &mut tokio::fs::File,
        cancellation: &AgentCancellationToken,
    ) -> Result<Option<String>, ImageArtifactError> {
        let download = async {
            let mut url =
                validate_artifact_url(initial_url, self.config.network_policy, &transfer_policy)?;
            for redirect_count in 0..=self.config.max_redirects {
                cancellation_check(cancellation)?;
                let resolved =
                    resolve_artifact_target(&url, self.config, &transfer_policy, cancellation)
                        .await?;
                let client = client_for_resolved_target(&resolved, self.config)?;
                let response = tokio::select! {
                    _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
                    response = client
                        .get(url.clone())
                        .header(ACCEPT, HeaderValue::from_static("image/png, image/jpeg, image/webp"))
                        .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
                        .send() => response.map_err(classify_transport_error)?,
                };
                verify_remote_address(&response, &resolved)?;
                if response.status().is_redirection() {
                    if redirect_count == self.config.max_redirects {
                        return Err(ImageArtifactError::new(
                            ImageArtifactErrorCode::RedirectRejected,
                            "image Artifact redirect limit was exceeded",
                            false,
                        ));
                    }
                    url = redirect_target(
                        &url,
                        response.status(),
                        response.headers(),
                        self.config.network_policy,
                        &transfer_policy,
                    )?;
                    continue;
                }
                if !response.status().is_success() {
                    return Err(ImageArtifactError::new(
                        ImageArtifactErrorCode::HttpRejected,
                        "image Artifact server returned an unsuccessful status",
                        response.status().is_server_error(),
                    )
                    .with_http_status(response.status().as_u16()));
                }
                validate_content_encoding(response.headers())?;
                if response.content_length().is_some_and(|length| {
                    length == 0 || length > self.config.max_download_bytes as u64
                }) {
                    return Err(ImageArtifactError::new(
                        ImageArtifactErrorCode::ResponseTooLarge,
                        "image Artifact exceeds the configured byte limit",
                        false,
                    ));
                }
                let content_type = normalized_content_type(response.headers());
                let mut total = 0usize;
                let mut stream = response.bytes_stream();
                while let Some(chunk) = tokio::select! {
                    _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
                    chunk = stream.next() => chunk,
                } {
                    let chunk = chunk.map_err(classify_transport_error)?;
                    total = total.checked_add(chunk.len()).ok_or_else(|| {
                        ImageArtifactError::new(
                            ImageArtifactErrorCode::ResponseTooLarge,
                            "image Artifact exceeds the configured byte limit",
                            false,
                        )
                    })?;
                    if total > self.config.max_download_bytes {
                        return Err(ImageArtifactError::new(
                            ImageArtifactErrorCode::ResponseTooLarge,
                            "image Artifact exceeds the configured byte limit",
                            false,
                        ));
                    }
                    staging.write_all(&chunk).await.map_err(io_error)?;
                }
                if total == 0 {
                    return Err(ImageArtifactError::new(
                        ImageArtifactErrorCode::InvalidImage,
                        "image Artifact response was empty",
                        false,
                    ));
                }
                return Ok(content_type);
            }
            Err(ImageArtifactError::new(
                ImageArtifactErrorCode::RedirectRejected,
                "image Artifact redirect could not be resolved",
                false,
            ))
        };
        tokio::select! {
            _ = cancellation.cancelled() => Err(ImageArtifactError::cancelled()),
            result = tokio::time::timeout(self.config.download_timeout, download) => {
                result.unwrap_or_else(|_| Err(ImageArtifactError::new(
                    ImageArtifactErrorCode::DownloadTimedOut,
                    "image Artifact download timed out",
                    true,
                )))
            }
        }
    }

    fn validate_candidate_path(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<PathBuf, ImageArtifactError> {
        let expected_name = format!("{}.{}", candidate.sha256, candidate.format.extension());
        let expected_relative = format!("{MANAGED_ARTIFACT_OBJECTS_DIRECTORY}/{expected_name}");
        if candidate.storage_relative_path != expected_relative
            || candidate.artifact_id != format!("sha256:{}", candidate.sha256)
            || !is_lower_hex_sha256(&candidate.sha256)
            || candidate.media_type != candidate.format.media_type()
            || candidate.size_bytes == 0
            || candidate.size_bytes > self.config.max_download_bytes as u64
            || candidate.width == 0
            || candidate.height == 0
            || candidate.width > MAX_IMAGE_ARTIFACT_DIMENSION
            || candidate.height > MAX_IMAGE_ARTIFACT_DIMENSION
            || u64::from(candidate.width)
                .checked_mul(u64::from(candidate.height))
                .is_none_or(|pixels| pixels > MAX_IMAGE_ARTIFACT_PIXELS)
        {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::Conflict,
                "image Artifact identity is invalid",
                false,
            ));
        }
        Ok(self.objects_root.join(expected_name))
    }
}

impl fmt::Debug for ManagedImageGenerationArtifactStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedImageGenerationArtifactStore")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ImageGenerationArtifactStore for ManagedImageGenerationArtifactStore {
    fn stage<'a>(
        &'a self,
        source: &'a ImageGenerationUrlOutput,
        transfer_policy: ImageArtifactTransferPolicy,
        cancellation: &'a AgentCancellationToken,
    ) -> BoxFuture<'a, Result<PreparedImageArtifact, ImageArtifactError>> {
        Box::pin(self.stage_inner(source, transfer_policy, cancellation))
    }

    fn publish(
        &self,
        mut prepared: PreparedImageArtifact,
        cancellation: &AgentCancellationToken,
    ) -> Result<PublishedImageArtifact, ImageArtifactError> {
        cancellation_check(cancellation)?;
        let expected_target = self.validate_candidate_path(&prepared.candidate)?;
        if expected_target != prepared.target_path {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::Conflict,
                "prepared image Artifact target identity changed",
                false,
            ));
        }
        verify_file_matches_candidate(
            &prepared.staging_path,
            &prepared.candidate,
            self.config.max_download_bytes,
        )?;
        let file = open_regular_file_no_follow(&prepared.staging_path).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        cancellation_check(cancellation)?;
        // Cancellation after this linearization point cannot turn a committed Artifact into a
        // cancelled result. The caller must finish authoritative reconciliation instead.
        match atomic_rename_noreplace(&prepared.staging_path, &prepared.target_path) {
            Ok(()) => {
                prepared.published = true;
                verify_file_matches_candidate(
                    &prepared.target_path,
                    &prepared.candidate,
                    self.config.max_download_bytes,
                )
                .map_err(|_| ImageArtifactError::commit_indeterminate())?;
                sync_directory(&self.objects_root)
                    .map_err(|_| ImageArtifactError::commit_indeterminate())?;
                Ok(PublishedImageArtifact {
                    candidate: prepared.candidate.clone(),
                    absolute_path: prepared.target_path.clone(),
                    status: ImageArtifactPublicationStatus::Created,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                verify_file_matches_candidate(
                    &prepared.target_path,
                    &prepared.candidate,
                    self.config.max_download_bytes,
                )?;
                sync_directory(&self.objects_root)
                    .map_err(|_| ImageArtifactError::commit_indeterminate())?;
                Ok(PublishedImageArtifact {
                    candidate: prepared.candidate.clone(),
                    absolute_path: prepared.target_path.clone(),
                    status: ImageArtifactPublicationStatus::AlreadyPresent,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                Err(ImageArtifactError::new(
                    ImageArtifactErrorCode::InvalidConfiguration,
                    "this platform cannot atomically publish image Artifacts",
                    false,
                ))
            }
            Err(error) => Err(io_error(error)),
        }
    }

    fn inspect(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError> {
        let target = self.validate_candidate_path(candidate)?;
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                verify_file_matches_candidate(&target, candidate, self.config.max_download_bytes)?;
                Ok(Some(PublishedImageArtifact {
                    candidate: candidate.clone(),
                    absolute_path: target,
                    status: ImageArtifactPublicationStatus::AlreadyPresent,
                }))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    fn confirm_published(
        &self,
        candidate: &ImageGenerationArtifactCandidate,
    ) -> Result<Option<PublishedImageArtifact>, ImageArtifactError> {
        let published = self.inspect(candidate)?;
        if published.is_some() {
            sync_directory(&self.objects_root)
                .map_err(|_| ImageArtifactError::commit_indeterminate())?;
        }
        Ok(published)
    }
}

struct StagingArtifactFile {
    path: PathBuf,
    file: File,
    armed: bool,
}

impl StagingArtifactFile {
    fn new(parent: &Path) -> Result<Self, ImageArtifactError> {
        for _ in 0..16 {
            let path = parent.join(format!(".staging-{}", Uuid::new_v4().simple()));
            let mut options = OpenOptions::new();
            options.create_new(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
                options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file,
                        armed: true,
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
        Err(ImageArtifactError::new(
            ImageArtifactErrorCode::Io,
            "could not allocate a private image Artifact staging file",
            true,
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingArtifactFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct ResolvedArtifactTarget {
    host: String,
    addresses: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
struct PublicDnsJsonResponse {
    #[serde(rename = "Status")]
    status: u16,
    #[serde(rename = "Answer", default)]
    answers: Vec<PublicDnsJsonAnswer>,
}

#[derive(Debug, Deserialize)]
struct PublicDnsJsonAnswer {
    #[serde(rename = "type")]
    record_type: u16,
    data: String,
}

async fn resolve_artifact_target(
    url: &Url,
    config: ImageArtifactStoreConfig,
    transfer_policy: &ImageArtifactTransferPolicy,
    cancellation: &AgentCancellationToken,
) -> Result<ResolvedArtifactTarget, ImageArtifactError> {
    let host = url
        .host_str()
        .ok_or_else(|| unsafe_url("image Artifact URL has no host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| unsafe_url("image Artifact URL uses an unsupported network scheme"))?;
    let mut system_lookup_failed = false;
    let addresses = if let Ok(address) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(address, port)]
    } else {
        let lookup = tokio::net::lookup_host((host, port));
        tokio::select! {
            _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
            result = tokio::time::timeout(config.dns_timeout, lookup) => {
                match result {
                    Ok(Ok(addresses)) => addresses.collect::<Vec<_>>(),
                    Ok(Err(_)) | Err(_) => {
                        system_lookup_failed = true;
                        Vec::new()
                    }
                }
            }
        }
    };
    let system_reported_fake_ip = addresses
        .iter()
        .any(|address| matches!(address.ip(), IpAddr::V4(value) if benchmark_fake_ipv4(value)));
    let mut addresses =
        approved_resolved_addresses(host, addresses, config.network_policy, transfer_policy);
    if addresses.is_empty()
        && (system_reported_fake_ip || system_lookup_failed)
        && config.network_policy == ImageArtifactNetworkPolicy::PublicHttpsOnly
        && fake_ip_eligible_dns_hostname(host)
    {
        addresses = resolve_through_public_dns(host, port, config, cancellation).await?;
    }
    if addresses.is_empty() {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::DnsRejected,
            "image Artifact host resolved to a disallowed network address",
            system_lookup_failed,
        ));
    }
    Ok(ResolvedArtifactTarget {
        host: host.to_string(),
        addresses,
    })
}

async fn resolve_through_public_dns(
    host: &str,
    port: u16,
    config: ImageArtifactStoreConfig,
    cancellation: &AgentCancellationToken,
) -> Result<Vec<SocketAddr>, ImageArtifactError> {
    let mut received_authoritative_response = false;
    let queries = join_all(
        PUBLIC_DNS_RESOLVERS
            .iter()
            .map(|resolver| query_public_dns_resolver(*resolver, host, config, cancellation)),
    );
    let results = tokio::select! {
        _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
        results = tokio::time::timeout(config.dns_timeout, queries) => {
            results.map_err(|_| public_dns_unavailable())?
        }
    };
    for result in results {
        match result {
            Ok(addresses) => {
                received_authoritative_response = true;
                let approved = addresses
                    .into_iter()
                    .filter(|address| {
                        address_allowed(*address, ImageArtifactNetworkPolicy::PublicHttpsOnly)
                    })
                    .map(|address| SocketAddr::new(address, port))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                if !approved.is_empty() {
                    return Ok(approved);
                }
            }
            Err(error) if error.code == ImageArtifactErrorCode::Cancelled => return Err(error),
            Err(_) => {}
        }
    }
    Err(ImageArtifactError::new(
        ImageArtifactErrorCode::DnsRejected,
        if received_authoritative_response {
            "image Artifact host was not verified to use a public network address"
        } else {
            "image Artifact public DNS verification was unavailable"
        },
        !received_authoritative_response,
    ))
}

async fn query_public_dns_resolver(
    resolver: PublicDnsResolver,
    host: &str,
    config: ImageArtifactStoreConfig,
    cancellation: &AgentCancellationToken,
) -> Result<Vec<IpAddr>, ImageArtifactError> {
    let resolver_addresses = resolver
        .addresses
        .iter()
        .filter_map(|address| address.parse::<IpAddr>().ok())
        .map(|address| SocketAddr::new(address, 443))
        .collect::<Vec<_>>();
    if resolver_addresses.is_empty() {
        return Err(public_dns_unavailable());
    }
    let client = Client::builder()
        .connect_timeout(config.connect_timeout.min(config.dns_timeout))
        .timeout(config.dns_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(resolver.host, &resolver_addresses)
        .build()
        .map_err(|_| public_dns_unavailable())?;
    let mut addresses = BTreeSet::new();
    let mut received_response = false;
    let (ipv4, ipv6) = join(
        query_public_dns_record(
            &client,
            resolver,
            &resolver_addresses,
            host,
            "A",
            1,
            cancellation,
        ),
        query_public_dns_record(
            &client,
            resolver,
            &resolver_addresses,
            host,
            "AAAA",
            28,
            cancellation,
        ),
    )
    .await;
    for result in [ipv4, ipv6] {
        match result {
            Ok(result) => {
                received_response = true;
                addresses.extend(result);
            }
            Err(error) if error.code == ImageArtifactErrorCode::Cancelled => return Err(error),
            Err(_) => {}
        }
    }
    if received_response {
        Ok(addresses.into_iter().collect())
    } else {
        Err(public_dns_unavailable())
    }
}

async fn query_public_dns_record(
    client: &Client,
    resolver: PublicDnsResolver,
    resolver_addresses: &[SocketAddr],
    host: &str,
    record_type: &str,
    expected_type: u16,
    cancellation: &AgentCancellationToken,
) -> Result<Vec<IpAddr>, ImageArtifactError> {
    let mut url =
        Url::parse(&format!("https://{}{}", resolver.host, resolver.path)).map_err(|_| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidConfiguration,
                "image Artifact public DNS resolver is invalid",
                false,
            )
        })?;
    url.query_pairs_mut()
        .append_pair("name", host)
        .append_pair("type", record_type);
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
        response = client
            .get(url)
            .header(
                ACCEPT,
                HeaderValue::from_static("application/dns-json, application/json"),
            )
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
            .send() => response.map_err(|_| public_dns_unavailable())?,
    };
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > MAX_PUBLIC_DNS_RESPONSE_BYTES as u64)
        || response
            .remote_addr()
            .is_none_or(|remote| !resolver_addresses.contains(&remote))
    {
        return Err(public_dns_unavailable());
    }
    let body = read_bounded_public_dns_body(response, cancellation).await?;
    let response = parse_public_dns_response(&body)?;
    if response.status != 0 {
        return Ok(Vec::new());
    }
    Ok(response
        .answers
        .into_iter()
        .filter(|answer| answer.record_type == expected_type)
        .filter_map(|answer| answer.data.parse::<IpAddr>().ok())
        .collect())
}

fn parse_public_dns_response(body: &[u8]) -> Result<PublicDnsJsonResponse, ImageArtifactError> {
    serde_json::from_slice(body).map_err(|_| public_dns_unavailable())
}

async fn read_bounded_public_dns_body(
    response: reqwest::Response,
    cancellation: &AgentCancellationToken,
) -> Result<Vec<u8>, ImageArtifactError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => return Err(ImageArtifactError::cancelled()),
        chunk = stream.next() => chunk,
    } {
        let chunk = chunk.map_err(|_| public_dns_unavailable())?;
        if body.len().saturating_add(chunk.len()) > MAX_PUBLIC_DNS_RESPONSE_BYTES {
            return Err(public_dns_unavailable());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn public_dns_unavailable() -> ImageArtifactError {
    ImageArtifactError::new(
        ImageArtifactErrorCode::DnsRejected,
        "image Artifact public DNS verification was unavailable",
        true,
    )
}

fn approved_resolved_addresses(
    host: &str,
    addresses: impl IntoIterator<Item = SocketAddr>,
    network_policy: ImageArtifactNetworkPolicy,
    transfer_policy: &ImageArtifactTransferPolicy,
) -> Vec<SocketAddr> {
    addresses
        .into_iter()
        .filter(|address| {
            address_allowed(address.ip(), network_policy)
                || fake_ip_address_allowed(*address, host, transfer_policy)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn client_for_resolved_target(
    target: &ResolvedArtifactTarget,
    config: ImageArtifactStoreConfig,
) -> Result<Client, ImageArtifactError> {
    Client::builder()
        .connect_timeout(config.connect_timeout)
        .timeout(config.download_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(&target.host, &target.addresses)
        .build()
        .map_err(|_| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidConfiguration,
                "image Artifact HTTP client could not be initialized",
                false,
            )
        })
}

fn verify_remote_address(
    response: &reqwest::Response,
    target: &ResolvedArtifactTarget,
) -> Result<(), ImageArtifactError> {
    let remote = response.remote_addr().ok_or_else(|| {
        ImageArtifactError::new(
            ImageArtifactErrorCode::DnsRejected,
            "image Artifact connection identity could not be verified",
            false,
        )
    })?;
    if target.addresses.contains(&remote) {
        Ok(())
    } else {
        Err(ImageArtifactError::new(
            ImageArtifactErrorCode::DnsRejected,
            "image Artifact connection did not use an approved network address",
            false,
        ))
    }
}

fn validate_artifact_url(
    value: &str,
    policy: ImageArtifactNetworkPolicy,
    transfer_policy: &ImageArtifactTransferPolicy,
) -> Result<Url, ImageArtifactError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_IMAGE_ARTIFACT_URL_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(unsafe_url("image Artifact URL is invalid"));
    }
    let url = Url::parse(value).map_err(|_| unsafe_url("image Artifact URL is invalid"))?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(unsafe_url(
            "image Artifact URL contains a disallowed component",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| unsafe_url("image Artifact URL has no host"))?;
    let loopback = literal_ip(host).is_some_and(|address| address.is_loopback())
        || host.eq_ignore_ascii_case("localhost");
    match url.scheme() {
        "https" => {}
        "http" if policy == ImageArtifactNetworkPolicy::AllowLoopbackHttpForTests && loopback => {}
        _ => return Err(unsafe_url("image Artifact URL must use HTTPS")),
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| unsafe_url("image Artifact URL uses an unsupported network scheme"))?;
    if policy == ImageArtifactNetworkPolicy::PublicHttpsOnly
        && port != 443
        && !transfer_policy.permits_configured_https_origin(host, port)
    {
        return Err(unsafe_url(
            "image Artifact URL must use HTTPS port 443 or the configured Provider origin port",
        ));
    }
    Ok(url)
}

fn redirect_target(
    current: &Url,
    status: StatusCode,
    headers: &HeaderMap,
    policy: ImageArtifactNetworkPolicy,
    transfer_policy: &ImageArtifactTransferPolicy,
) -> Result<Url, ImageArtifactError> {
    if !matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    ) {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::RedirectRejected,
            "image Artifact server returned an unsupported redirect",
            false,
        ));
    }
    let location = headers
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::RedirectRejected,
                "image Artifact redirect is missing its destination",
                false,
            )
        })?;
    let next = current.join(location).map_err(|_| {
        ImageArtifactError::new(
            ImageArtifactErrorCode::RedirectRejected,
            "image Artifact redirect destination is invalid",
            false,
        )
    })?;
    // Never return the destination in an error or Debug value: it may contain a signed query.
    validate_artifact_url(next.as_str(), policy, transfer_policy)
}

fn validate_content_encoding(headers: &HeaderMap) -> Result<(), ImageArtifactError> {
    if let Some(value) = headers.get(CONTENT_ENCODING) {
        let value = value.to_str().unwrap_or_default().trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("identity") {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::UnsupportedMediaType,
                "compressed image Artifact responses are not supported",
                false,
            ));
        }
    }
    Ok(())
}

fn normalized_content_type(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

struct ValidatedImage {
    format: ImageArtifactFormat,
    width: u32,
    height: u32,
    size_bytes: u64,
    sha256: String,
}

fn validate_staged_image(
    path: &Path,
    claimed_content_type: Option<&str>,
    max_bytes: usize,
    expected_size_bytes: Option<u64>,
) -> Result<ValidatedImage, ImageArtifactError> {
    read_and_validate_image(path, claimed_content_type, max_bytes, expected_size_bytes)
        .map(|(validated, _bytes)| validated)
}

fn read_and_validate_image(
    path: &Path,
    claimed_content_type: Option<&str>,
    max_bytes: usize,
    expected_size_bytes: Option<u64>,
) -> Result<(ValidatedImage, Vec<u8>), ImageArtifactError> {
    let mut file = open_regular_file_no_follow(path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > max_bytes as u64 {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::InvalidImage,
            "image Artifact staging file is empty, oversized, or not a regular file",
            false,
        ));
    }
    if expected_size_bytes.is_some_and(|expected| metadata.len() != expected) {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::Conflict,
            "image Artifact size does not match its frozen identity",
            false,
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::Conflict,
            "image Artifact changed while it was being validated",
            false,
        ));
    }
    let validated = validate_image_bytes(&bytes, claimed_content_type)?;
    Ok((validated, bytes))
}

fn validate_image_bytes(
    bytes: &[u8],
    claimed_content_type: Option<&str>,
) -> Result<ValidatedImage, ImageArtifactError> {
    let format = detect_image_format(bytes)?;
    if let Some(content_type) = claimed_content_type {
        if content_type != "application/octet-stream" && content_type != format.media_type() {
            return Err(ImageArtifactError::new(
                ImageArtifactErrorCode::UnsupportedMediaType,
                "image Artifact Content-Type does not match its bytes",
                false,
            ));
        }
    }
    let dimensions = ImageReader::with_format(Cursor::new(&bytes), format.image_format())
        .into_dimensions()
        .map_err(|_| {
            ImageArtifactError::new(
                ImageArtifactErrorCode::InvalidImage,
                "image Artifact dimensions could not be decoded",
                false,
            )
        })?;
    validate_image_dimensions(dimensions.0, dimensions.1)?;
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format.image_format());
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_ARTIFACT_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_ARTIFACT_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ARTIFACT_ALLOC_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| {
        ImageArtifactError::new(
            ImageArtifactErrorCode::InvalidImage,
            "image Artifact could not be fully decoded",
            false,
        )
    })?;
    let width = decoded.width();
    let height = decoded.height();
    validate_image_dimensions(width, height)?;
    let sha256 = hex_sha256(bytes);
    Ok(ValidatedImage {
        format,
        width,
        height,
        size_bytes: bytes.len() as u64,
        sha256,
    })
}

fn read_file_matching_candidate(
    path: &Path,
    candidate: &ImageGenerationArtifactCandidate,
    max_bytes: usize,
) -> Result<Vec<u8>, ImageArtifactError> {
    if candidate.size_bytes == 0 || candidate.size_bytes > max_bytes as u64 {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::Conflict,
            "image Artifact size is outside the managed store limit",
            false,
        ));
    }
    let (validated, bytes) = read_and_validate_image(
        path,
        Some(&candidate.media_type),
        max_bytes,
        Some(candidate.size_bytes),
    )?;
    if validated.format != candidate.format
        || validated.width != candidate.width
        || validated.height != candidate.height
        || validated.size_bytes != candidate.size_bytes
        || validated.sha256 != candidate.sha256
    {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::Conflict,
            "image Artifact content does not match its frozen identity",
            false,
        ));
    }
    Ok(bytes)
}

fn verify_file_matches_candidate(
    path: &Path,
    candidate: &ImageGenerationArtifactCandidate,
    max_bytes: usize,
) -> Result<(), ImageArtifactError> {
    read_file_matching_candidate(path, candidate, max_bytes).map(|_| ())
}

fn detect_image_format(bytes: &[u8]) -> Result<ImageArtifactFormat, ImageArtifactError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(ImageArtifactFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(ImageArtifactFormat::Jpeg)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Ok(ImageArtifactFormat::Webp)
    } else {
        Err(ImageArtifactError::new(
            ImageArtifactErrorCode::UnsupportedMediaType,
            "image Artifact is not a supported PNG, JPEG, or WebP image",
            false,
        ))
    }
}

fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "path is not a regular file",
        ));
    }
    Ok(file)
}

fn reject_symlink_or_non_directory(path: &Path) -> Result<(), ImageArtifactError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(ImageArtifactError::new(
            ImageArtifactErrorCode::InvalidConfiguration,
            "image Artifact store path is not a safe directory",
            false,
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), ImageArtifactError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), ImageArtifactError> {
    Ok(())
}

fn address_allowed(address: IpAddr, policy: ImageArtifactNetworkPolicy) -> bool {
    if policy == ImageArtifactNetworkPolicy::AllowLoopbackHttpForTests && address.is_loopback() {
        return true;
    }
    match address {
        IpAddr::V4(address) => public_ipv4(address),
        IpAddr::V6(address) => public_ipv6(address),
    }
}

fn fake_ip_address_allowed(
    address: SocketAddr,
    host: &str,
    transfer_policy: &ImageArtifactTransferPolicy,
) -> bool {
    transfer_policy.permits_fake_ip_for_origin(host, address.port())
        && matches!(address.ip(), IpAddr::V4(address) if benchmark_fake_ipv4(address))
}

fn benchmark_fake_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    let network = u32::from(Ipv4Addr::new(198, 18, 0, 0));
    value & (u32::MAX << 17) == network
}

fn public_ipv4(address: Ipv4Addr) -> bool {
    let value = u32::from(address);
    ![
        ("0.0.0.0", 8),
        ("10.0.0.0", 8),
        ("100.64.0.0", 10),
        ("127.0.0.0", 8),
        ("169.254.0.0", 16),
        ("172.16.0.0", 12),
        ("192.0.0.0", 24),
        ("192.0.2.0", 24),
        ("192.168.0.0", 16),
        ("198.18.0.0", 15),
        ("198.51.100.0", 24),
        ("203.0.113.0", 24),
        ("224.0.0.0", 4),
        ("240.0.0.0", 4),
    ]
    .into_iter()
    .any(|(network, prefix)| {
        let network = u32::from(network.parse::<Ipv4Addr>().expect("literal IPv4 network"));
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        value & mask == network & mask
    })
}

fn public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return public_ipv4(mapped);
    }
    let value = u128::from(address);
    let global_unicast = value & (u128::MAX << 125) == u128::from(0x2000_u16) << 112;
    if !global_unicast {
        return false;
    }
    let denied = [
        ("2001::", 32),
        ("2001:10::", 28),
        ("2001:20::", 28),
        ("2001:db8::", 32),
    ];
    !denied.into_iter().any(|(network, prefix)| {
        let network = u128::from(network.parse::<Ipv6Addr>().expect("literal IPv6 network"));
        let mask = u128::MAX << (128 - prefix);
        value & mask == network & mask
    })
}

fn literal_ip(host: &str) -> Option<IpAddr> {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

fn fake_ip_eligible_dns_hostname(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || host.ends_with('.')
        || literal_ip(host).is_some()
        || !host.contains('.')
    {
        return false;
    }
    let normalized = host.to_ascii_lowercase();
    if [
        "localhost",
        "local",
        "localdomain",
        "home",
        "home.arpa",
        "internal",
        "intranet",
        "lan",
        "corp",
        "invalid",
        "test",
        "example",
        "example.com",
        "example.net",
        "example.org",
        "onion",
        "arpa",
    ]
    .into_iter()
    .any(|suffix| {
        normalized == suffix
            || normalized
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }) {
        return false;
    }
    normalized.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
}

fn classify_transport_error(error: reqwest::Error) -> ImageArtifactError {
    if error.is_timeout() {
        return ImageArtifactError::new(
            ImageArtifactErrorCode::DownloadTimedOut,
            "image Artifact download timed out",
            true,
        );
    }
    ImageArtifactError::new(
        ImageArtifactErrorCode::TransportFailed,
        "image Artifact transport failed",
        error.is_connect(),
    )
}

fn unsafe_url(message: &'static str) -> ImageArtifactError {
    ImageArtifactError::new(ImageArtifactErrorCode::UnsafeUrl, message, false)
}

fn invalid_dimensions() -> ImageArtifactError {
    ImageArtifactError::new(
        ImageArtifactErrorCode::InvalidImage,
        "image Artifact dimensions exceed the safety limit",
        false,
    )
}

fn validate_image_dimensions(width: u32, height: u32) -> Result<(), ImageArtifactError> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(invalid_dimensions)?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_ARTIFACT_DIMENSION
        || height > MAX_IMAGE_ARTIFACT_DIMENSION
        || pixels > MAX_IMAGE_ARTIFACT_PIXELS
    {
        return Err(invalid_dimensions());
    }
    Ok(())
}

fn io_error(_error: io::Error) -> ImageArtifactError {
    ImageArtifactError::new(
        ImageArtifactErrorCode::Io,
        "image Artifact storage operation failed",
        true,
    )
}

fn cancellation_check(cancellation: &AgentCancellationToken) -> Result<(), ImageArtifactError> {
    if cancellation.is_cancelled() {
        Err(ImageArtifactError::cancelled())
    } else {
        Ok(())
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn png_bytes() -> Vec<u8> {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 3, image::Rgba([1, 2, 3, 255])));
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn serve_once(status: &str, headers: &[(&str, &str)], body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let headers = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect::<String>();
        let status = status.to_string();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        });
        format!("http://{address}/artifact?signature=private")
    }

    fn test_store(root: &Path) -> ManagedImageGenerationArtifactStore {
        ManagedImageGenerationArtifactStore::new(
            root,
            ImageArtifactStoreConfig {
                network_policy: ImageArtifactNetworkPolicy::AllowLoopbackHttpForTests,
                ..ImageArtifactStoreConfig::default()
            },
        )
        .unwrap()
    }

    fn seed_published_png(
        store: &ManagedImageGenerationArtifactStore,
    ) -> (ImageGenerationArtifactCandidate, PathBuf, Vec<u8>) {
        let bytes = png_bytes();
        let sha256 = hex_sha256(&bytes);
        let target = store.objects_root.join(format!("{sha256}.png"));
        fs::write(&target, &bytes).unwrap();
        let candidate = ImageGenerationArtifactCandidate {
            artifact_id: format!("sha256:{sha256}"),
            storage_relative_path: format!("objects/{sha256}.png"),
            format: ImageArtifactFormat::Png,
            media_type: "image/png".to_string(),
            width: 2,
            height: 3,
            size_bytes: bytes.len() as u64,
            sha256,
        };
        (candidate, target, bytes)
    }

    #[tokio::test]
    async fn downloads_validates_and_atomically_publishes_png() {
        let directory = tempdir().unwrap();
        let bytes = png_bytes();
        let url = serve_once("200 OK", &[("Content-Type", "image/png")], bytes.clone());
        let source = ImageGenerationUrlOutput::new(url.clone());
        let store = test_store(directory.path());

        let prepared = store
            .stage(
                &source,
                ImageArtifactTransferPolicy::public_https_only(),
                &AgentCancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(prepared.candidate().width, 2);
        assert_eq!(prepared.candidate().height, 3);
        assert!(!format!("{prepared:?}").contains(&url));
        let published = store
            .publish(prepared, &AgentCancellationToken::new())
            .unwrap();

        assert_eq!(published.status, ImageArtifactPublicationStatus::Created);
        assert_eq!(fs::read(&published.absolute_path).unwrap(), bytes);
        assert!(store.inspect(&published.candidate).unwrap().is_some());
        let content = store
            .read_published(&published.candidate)
            .await
            .unwrap()
            .expect("published Artifact should be readable");
        assert_eq!(content.candidate, published.candidate);
        assert_eq!(content.bytes, bytes);
    }

    #[tokio::test]
    async fn published_reads_fail_closed_for_missing_mismatched_and_corrupt_content() {
        let directory = tempdir().unwrap();
        let store = test_store(directory.path());
        let (candidate, target, _bytes) = seed_published_png(&store);

        let mut mismatched = candidate.clone();
        mismatched.width += 1;
        let error = store.read_published(&mismatched).await.unwrap_err();
        assert_eq!(error.code, ImageArtifactErrorCode::Conflict);

        fs::write(&target, b"not an image").unwrap();
        let error = store.read_published(&candidate).await.unwrap_err();
        assert!(matches!(
            error.code,
            ImageArtifactErrorCode::Conflict
                | ImageArtifactErrorCode::InvalidImage
                | ImageArtifactErrorCode::UnsupportedMediaType
        ));

        fs::remove_file(&target).unwrap();
        assert!(store.read_published(&candidate).await.unwrap().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn published_reads_reject_symlink_entries() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let store = test_store(directory.path());
        let (candidate, target, bytes) = seed_published_png(&store);
        let replacement = directory.path().join("replacement.png");
        fs::write(&replacement, bytes).unwrap();
        fs::remove_file(&target).unwrap();
        symlink(&replacement, &target).unwrap();

        let error = store.read_published(&candidate).await.unwrap_err();
        assert_eq!(error.code, ImageArtifactErrorCode::Conflict);
    }

    #[tokio::test]
    async fn mismatched_content_type_is_rejected_without_publication() {
        let directory = tempdir().unwrap();
        let url = serve_once("200 OK", &[("Content-Type", "text/html")], png_bytes());
        let source = ImageGenerationUrlOutput::new(url);
        let store = test_store(directory.path());

        let error = store
            .stage(
                &source,
                ImageArtifactTransferPolicy::public_https_only(),
                &AgentCancellationToken::new(),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, ImageArtifactErrorCode::UnsupportedMediaType);
        assert!(fs::read_dir(directory.path().join("objects"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn production_policy_rejects_private_and_special_networks() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "198.18.0.1",
            "203.0.113.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "2001:db8::1",
        ] {
            assert!(!address_allowed(
                address.parse().unwrap(),
                ImageArtifactNetworkPolicy::PublicHttpsOnly
            ));
        }
        assert!(address_allowed(
            "8.8.8.8".parse().unwrap(),
            ImageArtifactNetworkPolicy::PublicHttpsOnly
        ));
        assert!(address_allowed(
            "2606:4700:4700::1111".parse().unwrap(),
            ImageArtifactNetworkPolicy::PublicHttpsOnly
        ));
    }

    #[test]
    fn fake_ip_compatibility_requires_an_exact_adapter_owned_hostname() {
        const TRUSTED_HOST: &str = "ark-content-generation-v2-cn-beijing.tos-cn-beijing.volces.com";
        const TRUSTED_RULES: &[ImageArtifactHostRule] =
            &[ImageArtifactHostRule::ExactHttpsOrigin {
                host: TRUSTED_HOST,
                port: 443,
            }];
        let trusted = ImageArtifactTransferPolicy::with_trusted_fake_ip_https_hosts(TRUSTED_RULES);
        let fake_ip = "198.18.0.8:443".parse().unwrap();

        assert!(approved_resolved_addresses(
            TRUSTED_HOST,
            [fake_ip],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &trusted,
        )
        .contains(&fake_ip));
        assert!(approved_resolved_addresses(
            "untrusted.example",
            [fake_ip],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &trusted,
        )
        .is_empty());
        assert!(approved_resolved_addresses(
            &format!("{TRUSTED_HOST}.attacker.example"),
            [fake_ip],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &trusted,
        )
        .is_empty());
        assert!(approved_resolved_addresses(
            "198.18.0.8",
            [fake_ip],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &ImageArtifactTransferPolicy::with_trusted_fake_ip_https_hosts(&[
                ImageArtifactHostRule::ExactHttpsOrigin {
                    host: "198.18.0.8",
                    port: 443,
                },
            ]),
        )
        .is_empty());
    }

    #[test]
    fn configured_endpoint_host_grants_only_its_exact_fake_ip_origin() {
        let policy = ImageArtifactTransferPolicy::public_https_only()
            .with_configured_endpoint_https_origin("images.provider-one.com", 8443);
        let fake_ip_443 = "198.18.0.8:443".parse().unwrap();
        let fake_ip_8443 = "198.18.0.8:8443".parse().unwrap();

        assert_eq!(
            approved_resolved_addresses(
                "images.provider-one.com",
                [fake_ip_8443],
                ImageArtifactNetworkPolicy::PublicHttpsOnly,
                &policy,
            ),
            vec![fake_ip_8443],
        );
        assert!(approved_resolved_addresses(
            "images.provider-one.com",
            [fake_ip_443],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &policy,
        )
        .is_empty());
        assert!(validate_artifact_url(
            "https://images.provider-one.com:8443/result.png",
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &policy,
        )
        .is_ok());
        assert!(validate_artifact_url(
            "https://images.provider-one.com:9443/result.png",
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &policy,
        )
        .is_err());
        assert!(validate_artifact_url(
            "https://cdn.provider-one.com:8443/result.png",
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &policy,
        )
        .is_err());
        for host in [
            "localhost",
            "renderer",
            "images.local",
            "assets.home.arpa",
            "service.internal",
            "cdn.provider-one.com",
            "198.18.0.8",
        ] {
            assert!(
                approved_resolved_addresses(
                    host,
                    [fake_ip_8443],
                    ImageArtifactNetworkPolicy::PublicHttpsOnly,
                    &policy,
                )
                .is_empty(),
                "{host} must not receive Fake-IP compatibility"
            );
        }
    }

    #[test]
    fn public_dns_fallback_accepts_only_qualified_hosts_and_public_answers() {
        for host in [
            "images.provider-one.com",
            "signed-assets.provider-two.cn",
            "xn--fiqs8s.example-provider.net",
        ] {
            assert!(fake_ip_eligible_dns_hostname(host), "{host}");
        }
        for host in [
            "localhost",
            "renderer",
            "images.local",
            "assets.home.arpa",
            "service.internal",
            "cdn.example.com",
            "198.18.0.8",
            "bad_host.provider.com",
            "-bad.provider.com",
            "trailing.provider.com.",
        ] {
            assert!(!fake_ip_eligible_dns_hostname(host), "{host}");
        }

        let response = parse_public_dns_response(
            br#"{
                "Status": 0,
                "Answer": [
                    {"type": 5, "data": "cdn.provider.com."},
                    {"type": 1, "data": "203.0.113.10"},
                    {"type": 1, "data": "8.8.8.8"},
                    {"type": 28, "data": "2606:4700:4700::1111"}
                ]
            }"#,
        )
        .unwrap();
        let addresses = response
            .answers
            .into_iter()
            .filter_map(|answer| answer.data.parse::<IpAddr>().ok())
            .filter(|address| {
                address_allowed(*address, ImageArtifactNetworkPolicy::PublicHttpsOnly)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            addresses,
            vec![
                "8.8.8.8".parse::<IpAddr>().unwrap(),
                "2606:4700:4700::1111".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn fake_ip_compatibility_never_grants_other_private_networks() {
        const TRUSTED_HOST: &str = "ark-content-generation-v2-cn-beijing.tos-cn-beijing.volces.com";
        const TRUSTED_RULES: &[ImageArtifactHostRule] =
            &[ImageArtifactHostRule::ExactHttpsOrigin {
                host: TRUSTED_HOST,
                port: 443,
            }];
        let trusted = ImageArtifactTransferPolicy::with_trusted_fake_ip_https_hosts(TRUSTED_RULES);

        for address in ["10.0.0.1:443", "127.0.0.1:443", "169.254.169.254:443"] {
            assert!(approved_resolved_addresses(
                TRUSTED_HOST,
                [address.parse().unwrap()],
                ImageArtifactNetworkPolicy::PublicHttpsOnly,
                &trusted,
            )
            .is_empty());
        }
    }

    #[test]
    fn mixed_dns_pins_only_addresses_approved_for_the_current_host() {
        let public = "8.8.8.8:443".parse().unwrap();
        let private = "10.0.0.1:443".parse().unwrap();
        let fake_ip = "198.18.0.8:443".parse().unwrap();
        let approved = approved_resolved_addresses(
            "cdn.example",
            [private, public, fake_ip, public],
            ImageArtifactNetworkPolicy::PublicHttpsOnly,
            &ImageArtifactTransferPolicy::public_https_only(),
        );

        assert_eq!(approved, vec![public]);
    }

    #[tokio::test]
    async fn cancellation_removes_private_staging_file() {
        let directory = tempdir().unwrap();
        let store = test_store(directory.path());
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
        let source = ImageGenerationUrlOutput::new("http://127.0.0.1:9/a".to_string());

        assert_eq!(
            store
                .stage(
                    &source,
                    ImageArtifactTransferPolicy::public_https_only(),
                    &cancellation,
                )
                .await
                .unwrap_err()
                .code,
            ImageArtifactErrorCode::Cancelled
        );
        assert!(fs::read_dir(directory.path().join("objects"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn store_rejects_a_symlink_root() {
        let directory = tempdir().unwrap();
        let real = directory.path().join("real");
        fs::create_dir(&real).unwrap();
        let linked = directory.path().join("linked");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &linked).unwrap();
            let error = ManagedImageGenerationArtifactStore::new(
                &linked,
                ImageArtifactStoreConfig::default(),
            )
            .unwrap_err();
            assert_eq!(error.code, ImageArtifactErrorCode::InvalidConfiguration);
        }
    }

    #[test]
    fn inspect_rejects_oversized_or_replaced_artifact_before_reading_it() {
        let directory = tempdir().unwrap();
        let store = test_store(directory.path());
        let digest = "b".repeat(64);
        let target = directory
            .path()
            .join(MANAGED_ARTIFACT_OBJECTS_DIRECTORY)
            .join(format!("{digest}.png"));
        let file = File::create(&target).unwrap();
        file.set_len(DEFAULT_IMAGE_ARTIFACT_MAX_BYTES as u64 + 1)
            .unwrap();
        let candidate = ImageGenerationArtifactCandidate {
            artifact_id: format!("sha256:{digest}"),
            storage_relative_path: format!("{MANAGED_ARTIFACT_OBJECTS_DIRECTORY}/{digest}.png"),
            format: ImageArtifactFormat::Png,
            media_type: "image/png".to_string(),
            width: 2,
            height: 3,
            size_bytes: 4,
            sha256: digest,
        };

        let error = store.inspect(&candidate).unwrap_err();

        assert_eq!(error.code, ImageArtifactErrorCode::InvalidImage);
    }
}
