//! Credential-free acquisition provenance for managed Skill installations.
//!
//! Provenance is receipt metadata, never package content, trust, policy, a
//! filesystem capability, or model context. `authority` identifies the
//! immutable source that produced the installed bytes. Optional `refresh`
//! metadata is a separate, non-authoritative locator that a registered backend
//! adapter may re-resolve in a future installation transaction.

use super::model::{
    SkillInstallationId, SkillInstallationRevision, SkillRevision,
    SKILL_INSTALLATION_REVISION_PREFIX,
};
use super::origin::{SkillPackageOrigin, MAX_ORIGIN_PROVIDER_BYTES, MAX_ORIGIN_REFERENCE_BYTES};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{self, Write};

const INSTALLATION_REVISION_DOMAIN: &[u8] = b"mycopilot.skill.installation\0";
const INSTALLATION_REVISION_FORMAT_VERSION: u32 = 1;

/// Maximum serialized bytes retained for one adapter-owned provenance payload.
pub const MAX_SKILL_PROVENANCE_PAYLOAD_BYTES: usize = MAX_ORIGIN_REFERENCE_BYTES;

/// Immutable acquisition authority for the bytes committed by an installation.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationAuthority(ProvenanceRecord);

impl SkillInstallationAuthority {
    /// Core acquisition-adapter construction boundary.
    ///
    /// `payload` must contain only the provider's credential-free, validated
    /// authority document. It must not contain tokens, cookies, authorization
    /// headers, signed URLs, local paths, or other ambient capabilities.
    pub fn new(
        provider: impl Into<String>,
        schema_version: u32,
        payload: impl Into<String>,
    ) -> Result<Self, SkillInstallationProvenanceError> {
        ProvenanceRecord::new(provider, schema_version, payload).map(Self)
    }

    pub fn provider(&self) -> &str {
        self.0.provider()
    }

    pub fn schema_version(&self) -> u32 {
        self.0.schema_version()
    }

    /// Returns the adapter-owned, credential-free authority payload.
    ///
    /// Callers must not log this value or serialize it into client-facing DTOs.
    pub(crate) fn payload(&self) -> &str {
        self.0.payload()
    }

    pub(crate) fn adapter_view(&self) -> SkillInstallationAuthorityView<'_> {
        SkillInstallationAuthorityView { authority: self }
    }
}

impl fmt::Debug for SkillInstallationAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationAuthority")
            .field(&self.0)
            .finish()
    }
}

/// Borrowed, read-only authority metadata exposed only to the acquisition
/// adapter selected by the installation workflow.
///
/// The workflow constructs this capability for the duration of an adapter
/// callback. It cannot be constructed by consumers, does not own receipt
/// data, and must not be logged or serialized into client-facing DTOs.
#[derive(Clone, Copy)]
pub struct SkillInstallationAuthorityView<'a> {
    authority: &'a SkillInstallationAuthority,
}

impl SkillInstallationAuthorityView<'_> {
    pub fn provider(&self) -> &str {
        self.authority.provider()
    }

    pub fn schema_version(&self) -> u32 {
        self.authority.schema_version()
    }

    /// Returns the selected adapter's credential-free authority document.
    ///
    /// Adapters must decode and revalidate this value before use. They must
    /// not log it or forward it across the backend protocol boundary.
    pub fn payload(&self) -> &str {
        self.authority.payload()
    }
}

impl fmt::Debug for SkillInstallationAuthorityView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationAuthorityView")
            .field("provider", &self.provider())
            .field("schema_version", &self.schema_version())
            .field("payload", &"[redacted]")
            .field("payload_bytes", &self.payload().len())
            .finish()
    }
}

/// Re-resolvable acquisition locator retained independently from authority.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationRefresh(ProvenanceRecord);

impl SkillInstallationRefresh {
    /// Core acquisition-adapter construction boundary.
    ///
    /// The payload is data, not executable authority. A backend adapter must
    /// decode and revalidate it before every refresh. Credentials and ambient
    /// capabilities are forbidden.
    pub fn new(
        provider: impl Into<String>,
        schema_version: u32,
        payload: impl Into<String>,
    ) -> Result<Self, SkillInstallationProvenanceError> {
        ProvenanceRecord::new(provider, schema_version, payload).map(Self)
    }

    pub fn provider(&self) -> &str {
        self.0.provider()
    }

    pub fn schema_version(&self) -> u32 {
        self.0.schema_version()
    }

    /// Returns the adapter-owned, credential-free refresh payload.
    ///
    /// Callers must not log this value or serialize it into client-facing DTOs.
    pub(crate) fn payload(&self) -> &str {
        self.0.payload()
    }

    pub(crate) fn adapter_view(&self) -> SkillInstallationRefreshView<'_> {
        SkillInstallationRefreshView { refresh: self }
    }
}

impl fmt::Debug for SkillInstallationRefresh {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationRefresh")
            .field(&self.0)
            .finish()
    }
}

/// Borrowed, read-only refresh metadata exposed only to the acquisition
/// adapter selected by the installation workflow.
///
/// This is data, not executable authority. Adapters must strictly decode and
/// revalidate the payload on every refresh attempt.
#[derive(Clone, Copy)]
pub struct SkillInstallationRefreshView<'a> {
    refresh: &'a SkillInstallationRefresh,
}

impl SkillInstallationRefreshView<'_> {
    pub fn provider(&self) -> &str {
        self.refresh.provider()
    }

    pub fn schema_version(&self) -> u32 {
        self.refresh.schema_version()
    }

    /// Returns the selected adapter's credential-free refresh document.
    ///
    /// Adapters must not log this value or forward it across the backend
    /// protocol boundary.
    pub fn payload(&self) -> &str {
        self.refresh.payload()
    }
}

impl fmt::Debug for SkillInstallationRefreshView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationRefreshView")
            .field("provider", &self.provider())
            .field("schema_version", &self.schema_version())
            .field("payload", &"[redacted]")
            .field("payload_bytes", &self.payload().len())
            .finish()
    }
}

/// Receipt provenance with immutable authority and an optional refresh locator.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationProvenance {
    authority: SkillInstallationAuthority,
    refresh: Option<SkillInstallationRefresh>,
}

impl SkillInstallationProvenance {
    pub fn new(
        authority: SkillInstallationAuthority,
        refresh: Option<SkillInstallationRefresh>,
    ) -> Self {
        Self { authority, refresh }
    }

    /// Normalizes legacy audit metadata without granting it refresh authority.
    pub(crate) fn from_legacy_origin(origin: &SkillPackageOrigin) -> Self {
        let authority = SkillInstallationAuthority::new(origin.provider(), 1, origin.reference())
            .expect("validated legacy origins satisfy the provenance envelope");
        Self::new(authority, None)
    }

    pub fn authority(&self) -> &SkillInstallationAuthority {
        &self.authority
    }

    pub fn refresh(&self) -> Option<&SkillInstallationRefresh> {
        self.refresh.as_ref()
    }

    pub fn is_refreshable(&self) -> bool {
        self.refresh.is_some()
    }

    pub(crate) fn adapter_view(&self) -> SkillInstallationProvenanceView<'_> {
        SkillInstallationProvenanceView { provenance: self }
    }
}

impl fmt::Debug for SkillInstallationProvenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationProvenance")
            .field("authority", &self.authority)
            .field("refresh", &self.refresh)
            .finish()
    }
}

/// Borrowed, read-only provenance capability passed to the adapter selected
/// by the installation workflow for management presentation.
///
/// Unlike [`SkillInstallationProvenance`], this callback-scoped view exposes
/// adapter-owned payloads so an out-of-crate adapter can strictly decode its
/// own persisted schema. Its private construction keeps raw receipt payloads
/// out of general backend APIs.
#[derive(Clone, Copy)]
pub struct SkillInstallationProvenanceView<'a> {
    provenance: &'a SkillInstallationProvenance,
}

impl SkillInstallationProvenanceView<'_> {
    pub fn authority(&self) -> SkillInstallationAuthorityView<'_> {
        self.provenance.authority.adapter_view()
    }

    pub fn refresh(&self) -> Option<SkillInstallationRefreshView<'_>> {
        self.provenance
            .refresh
            .as_ref()
            .map(SkillInstallationRefresh::adapter_view)
    }
}

impl fmt::Debug for SkillInstallationProvenanceView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationProvenanceView")
            .field("authority", &self.authority())
            .field("refresh", &self.refresh())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ProvenanceRecord {
    provider: String,
    schema_version: u32,
    payload: String,
}

impl ProvenanceRecord {
    fn new(
        provider: impl Into<String>,
        schema_version: u32,
        payload: impl Into<String>,
    ) -> Result<Self, SkillInstallationProvenanceError> {
        let provider = provider.into();
        let payload = payload.into();
        validate_value(&provider, "provenance provider", MAX_ORIGIN_PROVIDER_BYTES)?;
        if schema_version == 0 {
            return Err(SkillInstallationProvenanceError::new(
                "provenance schema version must be greater than zero",
            ));
        }
        validate_value(
            &payload,
            "provenance payload",
            MAX_SKILL_PROVENANCE_PAYLOAD_BYTES,
        )?;
        Ok(Self {
            provider,
            schema_version,
            payload,
        })
    }

    fn provider(&self) -> &str {
        &self.provider
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn payload(&self) -> &str {
        &self.payload
    }

    fn update_digest(&self, digest: &mut Sha256) {
        update_bytes(digest, self.provider.as_bytes());
        digest.update(self.schema_version.to_be_bytes());
        update_bytes(digest, self.payload.as_bytes());
    }
}

impl fmt::Debug for ProvenanceRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProvenanceRecord")
            .field("provider", &self.provider)
            .field("schema_version", &self.schema_version)
            .field("payload", &"[redacted]")
            .field("payload_bytes", &self.payload.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillInstallationProvenanceError {
    reason: String,
}

impl SkillInstallationProvenanceError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillInstallationProvenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillInstallationProvenanceError {}

/// Derives the deterministic lifecycle CAS token for a normalized receipt.
pub(crate) fn installation_revision(
    installation_id: &SkillInstallationId,
    generation: u64,
    package_format_version: u32,
    package_revision: &SkillRevision,
    provenance: &SkillInstallationProvenance,
) -> SkillInstallationRevision {
    let mut digest = Sha256::new();
    digest.update(INSTALLATION_REVISION_DOMAIN);
    digest.update(INSTALLATION_REVISION_FORMAT_VERSION.to_be_bytes());
    update_bytes(&mut digest, installation_id.as_str().as_bytes());
    digest.update(generation.to_be_bytes());
    digest.update(package_format_version.to_be_bytes());
    update_bytes(&mut digest, package_revision.as_str().as_bytes());
    update_bytes(&mut digest, b"authority");
    provenance.authority.0.update_digest(&mut digest);
    match &provenance.refresh {
        Some(refresh) => {
            digest.update([1]);
            update_bytes(&mut digest, b"refresh");
            refresh.0.update_digest(&mut digest);
        }
        None => digest.update([0]),
    }
    SkillInstallationRevision::trusted(format_digest(digest.finalize()))
}

fn validate_value(
    value: &str,
    label: &str,
    max_bytes: usize,
) -> Result<(), SkillInstallationProvenanceError> {
    if value.is_empty() {
        return Err(SkillInstallationProvenanceError::new(format!(
            "{label} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(SkillInstallationProvenanceError::new(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(SkillInstallationProvenanceError::new(format!(
            "{label} contains a control character"
        )));
    }
    Ok(())
}

fn update_bytes(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

fn format_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut revision =
        String::with_capacity(SKILL_INSTALLATION_REVISION_PREFIX.len() + digest.len() * 2);
    revision.push_str(SKILL_INSTALLATION_REVISION_PREFIX);
    for byte in digest {
        write!(&mut revision, "{byte:02x}").expect("writing to a String cannot fail");
    }
    revision
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installation_id() -> SkillInstallationId {
        SkillInstallationId::parse("01234567-89ab-4def-8123-456789abcdef").unwrap()
    }

    fn package_revision() -> SkillRevision {
        SkillRevision::parse(format!("skill-package-sha256-v1:{}", "a".repeat(64))).unwrap()
    }

    fn provenance(refresh_payload: Option<&str>) -> SkillInstallationProvenance {
        SkillInstallationProvenance::new(
            SkillInstallationAuthority::new("github", 1, "immutable-authority").unwrap(),
            refresh_payload
                .map(|payload| SkillInstallationRefresh::new("github", 1, payload).unwrap()),
        )
    }

    #[test]
    fn provenance_is_bounded_and_rejects_control_characters() {
        assert!(SkillInstallationAuthority::new("", 1, "payload").is_err());
        assert!(SkillInstallationAuthority::new("github", 0, "payload").is_err());
        assert!(SkillInstallationAuthority::new("github\n", 1, "payload").is_err());
        assert!(SkillInstallationAuthority::new("github", 1, "secret\0payload").is_err());
        assert!(SkillInstallationAuthority::new(
            "github",
            1,
            "p".repeat(MAX_SKILL_PROVENANCE_PAYLOAD_BYTES),
        )
        .is_ok());
        assert!(SkillInstallationAuthority::new(
            "github",
            1,
            "p".repeat(MAX_SKILL_PROVENANCE_PAYLOAD_BYTES + 1),
        )
        .is_err());
    }

    #[test]
    fn debug_output_never_reveals_payloads() {
        let secret_canary = "credential-canary-must-not-leak";
        let value = provenance(Some(secret_canary));
        let output = format!("{value:?}");
        assert!(!output.contains(secret_canary));
        assert!(output.contains("[redacted]"));
        assert!(output.contains("github"));
    }

    #[test]
    fn installation_revision_binds_generation_package_and_both_provenance_roles() {
        let id = installation_id();
        let package = package_revision();
        let base = provenance(None);
        let refresh_a = provenance(Some("main"));
        let refresh_b = provenance(Some("stable"));

        let first = installation_revision(&id, 1, 1, &package, &base);
        assert_eq!(first, installation_revision(&id, 1, 1, &package, &base));
        assert_ne!(first, installation_revision(&id, 2, 1, &package, &base));
        assert_ne!(first, installation_revision(&id, 1, 2, &package, &base));
        assert_ne!(
            first,
            installation_revision(&id, 1, 1, &package, &refresh_a)
        );
        assert_ne!(
            installation_revision(&id, 1, 1, &package, &refresh_a),
            installation_revision(&id, 1, 1, &package, &refresh_b)
        );
        assert!(SkillInstallationRevision::parse(first.as_str()).is_ok());
    }

    #[test]
    fn installation_revision_parser_is_exact() {
        let valid = format!("{SKILL_INSTALLATION_REVISION_PREFIX}{}", "a".repeat(64));
        assert!(SkillInstallationRevision::parse(valid).is_ok());
        assert!(SkillInstallationRevision::parse(format!(
            "{SKILL_INSTALLATION_REVISION_PREFIX}{}",
            "A".repeat(64)
        ))
        .is_err());
        assert!(SkillInstallationRevision::parse(format!(
            "{SKILL_INSTALLATION_REVISION_PREFIX}{}",
            "a".repeat(63)
        ))
        .is_err());
        assert!(SkillInstallationRevision::parse(format!(
            "skill-installation-sha256-v2:{}",
            "a".repeat(64)
        ))
        .is_err());
    }
}
