//! Immutable output shared by every Skill acquisition adapter.
//!
//! Package bytes and receipt provenance are one value so preview, retry,
//! source-candidate handoff, and commit cannot accidentally pair bytes with
//! metadata from different acquisitions.

use super::acquisition_provenance::SkillInstallationProvenance;
use super::prepared::PreparedSkillPackage;
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSkillAcquisition {
    package: PreparedSkillPackage,
    provenance: SkillInstallationProvenance,
}

impl PreparedSkillAcquisition {
    /// Binds validated package bytes to the credential-free provenance that
    /// a trusted acquisition adapter wants committed with those exact bytes.
    ///
    /// External adapters may construct this value, but must never place
    /// credentials, signed URLs, local paths, or ambient capabilities in the
    /// provenance payloads.
    pub fn new(package: PreparedSkillPackage, provenance: SkillInstallationProvenance) -> Self {
        Self {
            package,
            provenance,
        }
    }

    pub fn package(&self) -> &PreparedSkillPackage {
        &self.package
    }

    pub fn provenance(&self) -> &SkillInstallationProvenance {
        &self.provenance
    }

    /// Returns whether every provider-owned part of this acquisition belongs
    /// to the provider whose adapter or resolver produced it.
    ///
    /// This is deliberately crate-private: external adapters describe their
    /// output, while the trusted workflow and source-resolution boundaries
    /// enforce the binding. Cross-provider delegation is not supported; it
    /// will require an explicit authority model rather than accepting mixed
    /// provider metadata implicitly.
    pub(crate) fn is_owned_by(&self, provider: &str) -> bool {
        self.package.origin().provider() == provider
            && self.provenance.authority().provider() == provider
            && self
                .provenance
                .refresh()
                .is_none_or(|refresh| refresh.provider() == provider)
    }

    pub(crate) fn into_parts(self) -> (PreparedSkillPackage, SkillInstallationProvenance) {
        (self.package, self.provenance)
    }

    pub(super) fn retained_payload_bytes(&self) -> usize {
        let authority = self.provenance.authority();
        let authority_bytes = authority
            .provider()
            .len()
            .saturating_add(authority.payload().len());
        let refresh_bytes = self.provenance.refresh().map_or(0, |refresh| {
            refresh
                .provider()
                .len()
                .saturating_add(refresh.payload().len())
        });
        self.package
            .retained_payload_bytes()
            .saturating_add(authority_bytes)
            .saturating_add(refresh_bytes)
    }
}

impl fmt::Debug for PreparedSkillAcquisition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedSkillAcquisition")
            .field("package", &self.package)
            .field("provenance", &self.provenance)
            .finish()
    }
}
