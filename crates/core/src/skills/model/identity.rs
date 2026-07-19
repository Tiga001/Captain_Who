use super::*;

pub const SKILL_PACKAGE_FORMAT_VERSION: u32 = 1;
pub const SKILL_PACKAGE_FORMAT_VERSION_V2: u32 = 2;
pub const SKILL_PACKAGE_FORMAT_VERSION_V3: u32 = 3;
pub const DEFAULT_MAX_ACTIVATED_SKILLS: usize = 8;
pub const DEFAULT_MAX_ACTIVATED_SKILL_BYTES: usize = 512 * 1024;

const MAX_OPAQUE_ID_BYTES: usize = 16 * 1024;
const MAX_LOCAL_SKILL_ID_BYTES: usize = 4 * 1024;
const MAX_SOURCE_ID_BYTES: usize = MAX_OPAQUE_ID_BYTES - MAX_LOCAL_SKILL_ID_BYTES - 1;
const MAX_REVISION_BYTES: usize = 256;

pub const SKILL_INSTALLATION_REVISION_PREFIX: &str = "skill-installation-sha256-v1:";

/// Stable identity of one configured Skill source.
///
/// The value is opaque to callers. Its final `:`-separated component is part
/// of the source identity, while a [`SkillId`] appends one local component.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillSourceId(String);

impl SkillSourceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "source id", MAX_SOURCE_ID_BYTES)?;
        let Some((kind, authority)) = value.split_once(':') else {
            return Err(SkillReferenceError::new(
                "source id must contain a non-empty kind and authority",
            ));
        };
        if kind.is_empty() || authority.is_empty() || authority.split(':').any(str::is_empty) {
            return Err(SkillReferenceError::new(
                "source id must contain non-empty colon-separated kind and authority components",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillSourceId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillSourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillSourceId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Stable application-generated identity of one managed Skill installation.
///
/// The identity is deliberately independent from the package revision so an
/// installation can move to a new immutable package without changing its
/// source-qualified [`SkillId`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillInstallationId(String);

impl SkillInstallationId {
    /// Generates a stable id for an installation request. Callers should keep
    /// and retry the same request if commit acknowledgement is uncertain.
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        let uuid = Uuid::parse_str(&value)
            .map_err(|_| SkillReferenceError::new("installation id must be a UUID"))?;
        if uuid.is_nil() {
            return Err(SkillReferenceError::new(
                "installation id must not be the nil UUID",
            ));
        }
        if uuid.hyphenated().to_string() != value {
            return Err(SkillReferenceError::new(
                "installation id must use canonical lowercase hyphenated UUID form",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SkillInstallationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillInstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillInstallationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillInstallationId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Opaque, source-qualified Skill identity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillId {
    value: String,
    source_id: SkillSourceId,
}

impl SkillId {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "skill id", MAX_OPAQUE_ID_BYTES)?;
        let (source, local_id) = value
            .rsplit_once(':')
            .ok_or_else(|| SkillReferenceError::new("skill id is missing its local component"))?;
        if local_id.is_empty() || local_id.contains(':') {
            return Err(SkillReferenceError::new(
                "skill id must have one non-empty local component",
            ));
        }
        validate_opaque_ascii(local_id, "local skill id", MAX_LOCAL_SKILL_ID_BYTES)?;
        let source_id = SkillSourceId::parse(source)?;
        Ok(Self { value, source_id })
    }

    pub(crate) fn from_parts(
        source_id: SkillSourceId,
        local_id: &str,
    ) -> Result<Self, SkillReferenceError> {
        validate_opaque_ascii(local_id, "local skill id", MAX_LOCAL_SKILL_ID_BYTES)?;
        if local_id.contains(':') {
            return Err(SkillReferenceError::new(
                "local skill id cannot contain a colon",
            ));
        }
        Self::parse(format!("{source_id}:{local_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn source_id(&self) -> &SkillSourceId {
        &self.source_id
    }

    pub(crate) fn local_id(&self) -> &str {
        self.value
            .rsplit_once(':')
            .map_or("", |(_, local_id)| local_id)
    }
}

impl fmt::Debug for SkillId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SkillId").field(&self.value).finish()
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(formatter)
    }
}

impl FromStr for SkillId {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillId {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// Revision token binding a selection to one exact Skill package snapshot.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillRevision(String);

impl SkillRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        validate_opaque_ascii(&value, "skill revision", MAX_REVISION_BYTES)?;
        Ok(Self(value))
    }

    pub(crate) fn trusted(value: String) -> Self {
        debug_assert!(Self::parse(value.clone()).is_ok());
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillRevision {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillRevision {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillRevision {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// Compare-and-swap identity for one exact managed installation receipt.
///
/// Unlike [`SkillRevision`], this token binds installation lifecycle state,
/// including its monotonically increasing generation and acquisition
/// provenance. It therefore changes even when an update retains identical
/// package bytes but changes where future refreshes are resolved from.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillInstallationRevision(String);

impl SkillInstallationRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillReferenceError> {
        let value = value.into();
        let Some(digest) = value.strip_prefix(SKILL_INSTALLATION_REVISION_PREFIX) else {
            return Err(SkillReferenceError::new(format!(
                "installation revision must start with `{SKILL_INSTALLATION_REVISION_PREFIX}`"
            )));
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(SkillReferenceError::new(
                "installation revision digest must contain exactly 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    pub(crate) fn trusted(value: String) -> Self {
        debug_assert!(Self::parse(value.clone()).is_ok());
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillInstallationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillInstallationRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillInstallationRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SkillInstallationRevision {
    type Err = SkillReferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl PartialEq<str> for SkillInstallationRevision {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<String> for SkillInstallationRevision {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

/// A user or model selection captured from a catalog.
///
/// Both fields are validated and immutable, so a catalog descriptor can always
/// be converted to a resolver input without a second, weaker string contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillSelection {
    skill_id: SkillId,
    expected_revision: SkillRevision,
}

impl SkillSelection {
    pub fn new(skill_id: SkillId, expected_revision: SkillRevision) -> Self {
        Self {
            skill_id,
            expected_revision,
        }
    }

    pub fn parse(
        skill_id: impl Into<String>,
        expected_revision: impl Into<String>,
    ) -> Result<Self, SkillReferenceError> {
        Ok(Self::new(
            SkillId::parse(skill_id)?,
            SkillRevision::parse(expected_revision)?,
        ))
    }

    pub fn from_descriptor(descriptor: &SkillDescriptor) -> Self {
        Self::new(descriptor.id().clone(), descriptor.revision().clone())
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> &SkillRevision {
        &self.expected_revision
    }
}

/// Compatibility name for the v1 resolver request.
pub type SkillResolveRequest = SkillSelection;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillReferenceError {
    reason: String,
}

impl SkillReferenceError {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillReferenceError {}

fn validate_opaque_ascii(
    value: &str,
    label: &str,
    max_bytes: usize,
) -> Result<(), SkillReferenceError> {
    if value.is_empty() {
        return Err(SkillReferenceError::new(format!(
            "{label} must not be empty"
        )));
    }
    if value.len() > max_bytes {
        return Err(SkillReferenceError::new(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(SkillReferenceError::new(format!(
            "{label} must contain only printable ASCII without whitespace"
        )));
    }
    Ok(())
}
