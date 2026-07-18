use std::error::Error;
use std::fmt;

pub(super) const MAX_ORIGIN_PROVIDER_BYTES: usize = 128;
pub(super) const MAX_ORIGIN_REFERENCE_BYTES: usize = 4 * 1024;

/// Bounded, non-authoritative audit metadata describing how a package was
/// acquired. Origin values never participate in store paths, trust, policy, or
/// model context.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillPackageOrigin {
    provider: String,
    reference: String,
}

impl fmt::Debug for SkillPackageOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillPackageOrigin")
            .field("provider", &self.provider)
            .field("reference", &"[redacted]")
            .field("reference_bytes", &self.reference.len())
            .finish()
    }
}

impl SkillPackageOrigin {
    pub fn new(
        provider: impl Into<String>,
        reference: impl Into<String>,
    ) -> Result<Self, SkillPackageOriginError> {
        let provider = provider.into();
        let reference = reference.into();
        validate_origin_value(&provider, "origin provider", MAX_ORIGIN_PROVIDER_BYTES)?;
        validate_origin_value(&reference, "origin reference", MAX_ORIGIN_REFERENCE_BYTES)?;
        Ok(Self {
            provider,
            reference,
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn reference(&self) -> &str {
        &self.reference
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillPackageOriginError {
    reason: String,
}

impl SkillPackageOriginError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPackageOriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPackageOriginError {}

fn validate_origin_value(
    value: &str,
    label: &str,
    max_bytes: usize,
) -> Result<(), SkillPackageOriginError> {
    if value.is_empty() {
        return Err(SkillPackageOriginError::new(format!(
            "{label} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(SkillPackageOriginError::new(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(SkillPackageOriginError::new(format!(
            "{label} contains a control character"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_limits_are_inclusive() {
        assert!(SkillPackageOrigin::new(
            "p".repeat(MAX_ORIGIN_PROVIDER_BYTES),
            "r".repeat(MAX_ORIGIN_REFERENCE_BYTES),
        )
        .is_ok());
        assert!(
            SkillPackageOrigin::new("p".repeat(MAX_ORIGIN_PROVIDER_BYTES + 1), "reference",)
                .is_err()
        );
        assert!(
            SkillPackageOrigin::new("provider", "r".repeat(MAX_ORIGIN_REFERENCE_BYTES + 1),)
                .is_err()
        );
    }

    #[test]
    fn origins_reject_empty_and_control_values() {
        assert!(SkillPackageOrigin::new("", "reference").is_err());
        assert!(SkillPackageOrigin::new("provider", "").is_err());
        assert!(SkillPackageOrigin::new("provider\n", "reference").is_err());
        assert!(SkillPackageOrigin::new("provider", "secret\0reference").is_err());
    }
}
