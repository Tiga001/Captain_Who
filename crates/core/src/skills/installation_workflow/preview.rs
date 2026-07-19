use super::*;

pub(super) const PREVIEW_REVISION_PREFIX: &str = "skill-install-preview-sha256-v1:";

/// Revision of the exact preview contract accepted by a commit.
///
/// This is distinct from [`SkillRevision`]: a package revision identifies the
/// package bytes used by the installer's CAS, while a preview revision also
/// binds the operation target, acquisition request, warnings, and expiration.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPreviewRevision(String);

impl SkillPreviewRevision {
    pub fn parse(value: impl Into<String>) -> Result<Self, SkillPreviewRevisionError> {
        let value = value.into();
        let digest = value
            .strip_prefix(PREVIEW_REVISION_PREFIX)
            .ok_or_else(|| SkillPreviewRevisionError::new("invalid preview revision prefix"))?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(SkillPreviewRevisionError::new(
                "preview revision digest must contain 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    pub(super) fn from_digest(digest: [u8; 32]) -> Self {
        let mut value = String::with_capacity(PREVIEW_REVISION_PREFIX.len() + 64);
        value.push_str(PREVIEW_REVISION_PREFIX);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SkillPreviewRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillPreviewRevision")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillPreviewRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPreviewRevisionError {
    pub(super) reason: String,
}

impl SkillPreviewRevisionError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPreviewRevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPreviewRevisionError {}

/// Client-generated idempotency identity for one prepare/commit conversation.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPreparationId(String);

impl SkillPreparationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, SkillPreparationIdError> {
        let value = value.into();
        let uuid = Uuid::parse_str(&value)
            .map_err(|_| SkillPreparationIdError::new("preparation id must be a UUID"))?;
        if uuid.is_nil() {
            return Err(SkillPreparationIdError::new(
                "preparation id must not be the nil UUID",
            ));
        }
        if uuid.hyphenated().to_string() != value {
            return Err(SkillPreparationIdError::new(
                "preparation id must use canonical lowercase hyphenated UUID form",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for SkillPreparationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SkillPreparationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SkillPreparationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for SkillPreparationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPreparationIdError {
    pub(super) reason: String,
}

impl SkillPreparationIdError {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for SkillPreparationIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(formatter)
    }
}

impl Error for SkillPreparationIdError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillInstallationPreparationIntent {
    Install {
        installation_id: SkillInstallationId,
    },
    Update {
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
    },
}

impl SkillInstallationPreparationIntent {
    pub fn operation(&self) -> SkillInstallationOperation {
        match self {
            Self::Install { .. } => SkillInstallationOperation::Install,
            Self::Update { .. } => SkillInstallationOperation::Update,
        }
    }
}

/// Immutable identity-bound inspection request.
#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPreparationRequest {
    pub(super) preparation_id: SkillPreparationId,
    pub(super) intent: SkillInstallationPreparationIntent,
    pub(super) source: SkillAcquisitionSource,
}

impl SkillInstallationPreparationRequest {
    pub fn install(
        preparation_id: SkillPreparationId,
        installation_id: SkillInstallationId,
        source: SkillAcquisitionSource,
    ) -> Self {
        Self {
            preparation_id,
            intent: SkillInstallationPreparationIntent::Install { installation_id },
            source,
        }
    }

    pub fn update(
        preparation_id: SkillPreparationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
        source: SkillAcquisitionSource,
    ) -> Self {
        Self {
            preparation_id,
            intent: SkillInstallationPreparationIntent::Update {
                skill_id,
                expected_revision,
            },
            source,
        }
    }

    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn intent(&self) -> &SkillInstallationPreparationIntent {
        &self.intent
    }

    pub fn source(&self) -> &SkillAcquisitionSource {
        &self.source
    }
}

impl fmt::Debug for SkillInstallationPreparationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPreparationRequest")
            .field("preparation_id", &self.preparation_id)
            .field("intent", &self.intent)
            .field("source", &self.source)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum SkillInstallationWarningCode {
    ContainsScripts,
    ResourcesNotExposed,
}

impl SkillInstallationWarningCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ContainsScripts => "containsScripts",
            Self::ResourcesNotExposed => "resourcesNotExposed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationWarning {
    pub(super) code: SkillInstallationWarningCode,
    pub(super) message: String,
    pub(super) acknowledgement_required: bool,
}

impl SkillInstallationWarning {
    pub fn code(&self) -> SkillInstallationWarningCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn acknowledgement_required(&self) -> bool {
        self.acknowledgement_required
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPackageResourceSummary {
    pub(super) resource_count: usize,
    pub(super) resource_bytes: u64,
    pub(super) reference_count: usize,
    pub(super) asset_count: usize,
    pub(super) script_count: usize,
}

impl SkillPackageResourceSummary {
    pub fn resource_count(&self) -> usize {
        self.resource_count
    }

    pub fn resource_bytes(&self) -> u64 {
        self.resource_bytes
    }

    pub fn reference_count(&self) -> usize {
        self.reference_count
    }

    pub fn asset_count(&self) -> usize {
        self.asset_count
    }

    pub fn script_count(&self) -> usize {
        self.script_count
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPackagePreview {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) revision: SkillRevision,
    pub(super) format_version: u32,
    pub(super) entrypoint_bytes: u64,
    pub(super) resources: SkillPackageResourceSummary,
}

impl SkillInstallationPackagePreview {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn revision(&self) -> &SkillRevision {
        &self.revision
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn entrypoint_bytes(&self) -> u64 {
        self.entrypoint_bytes
    }

    pub fn resources(&self) -> &SkillPackageResourceSummary {
        &self.resources
    }

    pub fn package_bytes(&self) -> u64 {
        self.entrypoint_bytes
            .saturating_add(self.resources.resource_bytes)
    }
}

impl fmt::Debug for SkillInstallationPackagePreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPackagePreview")
            .field("name", &self.name)
            .field("revision", &self.revision)
            .field("format_version", &self.format_version)
            .field("entrypoint_bytes", &self.entrypoint_bytes)
            .field("resources", &self.resources)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SkillInstallationPreview {
    pub(super) preparation_id: SkillPreparationId,
    pub(super) preview_revision: SkillPreviewRevision,
    pub(super) operation: SkillInstallationOperation,
    pub(super) installation_id: SkillInstallationId,
    pub(super) skill_id: SkillId,
    pub(super) expected_revision: Option<SkillInstallationRevision>,
    pub(super) content_changed: bool,
    pub(super) source_changed: bool,
    pub(super) acquisition: SkillAcquisitionPresentation,
    pub(super) package: SkillInstallationPackagePreview,
    pub(super) warnings: Arc<[SkillInstallationWarning]>,
    pub(super) expires_at_unix_ms: u64,
}

impl SkillInstallationPreview {
    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn preview_revision(&self) -> &SkillPreviewRevision {
        &self.preview_revision
    }

    pub fn operation(&self) -> SkillInstallationOperation {
        self.operation
    }

    pub fn installation_id(&self) -> &SkillInstallationId {
        &self.installation_id
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn expected_revision(&self) -> Option<&SkillInstallationRevision> {
        self.expected_revision.as_ref()
    }

    pub fn content_changed(&self) -> bool {
        self.content_changed
    }

    pub fn source_changed(&self) -> bool {
        self.source_changed
    }

    pub fn acquisition(&self) -> &SkillAcquisitionPresentation {
        &self.acquisition
    }

    pub fn package(&self) -> &SkillInstallationPackagePreview {
        &self.package
    }

    pub fn warnings(&self) -> &[SkillInstallationWarning] {
        &self.warnings
    }

    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

impl fmt::Debug for SkillInstallationPreview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillInstallationPreview")
            .field("preparation_id", &self.preparation_id)
            .field("preview_revision", &self.preview_revision)
            .field("operation", &self.operation)
            .field("installation_id", &self.installation_id)
            .field("skill_id", &self.skill_id)
            .field("expected_revision", &self.expected_revision)
            .field("content_changed", &self.content_changed)
            .field("source_changed", &self.source_changed)
            .field("acquisition", &self.acquisition)
            .field("package", &self.package)
            .field("warnings", &self.warnings)
            .field("expires_at_unix_ms", &self.expires_at_unix_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationCommitRequest {
    pub(super) preparation_id: SkillPreparationId,
    pub(super) expected_preview_revision: SkillPreviewRevision,
    pub(super) acknowledged_warnings: BTreeSet<SkillInstallationWarningCode>,
}

impl SkillInstallationCommitRequest {
    pub fn new(
        preparation_id: SkillPreparationId,
        expected_preview_revision: SkillPreviewRevision,
    ) -> Self {
        Self {
            preparation_id,
            expected_preview_revision,
            acknowledged_warnings: BTreeSet::new(),
        }
    }

    pub fn acknowledge(mut self, code: SkillInstallationWarningCode) -> Self {
        self.acknowledged_warnings.insert(code);
        self
    }

    pub fn preparation_id(&self) -> &SkillPreparationId {
        &self.preparation_id
    }

    pub fn expected_preview_revision(&self) -> &SkillPreviewRevision {
        &self.expected_preview_revision
    }

    pub fn acknowledged_warnings(&self) -> &BTreeSet<SkillInstallationWarningCode> {
        &self.acknowledged_warnings
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInstallationCommitResult {
    pub(super) preview: SkillInstallationPreview,
    pub(super) mutation: SkillInstallationMutation,
    pub(super) replayed: bool,
}

impl SkillInstallationCommitResult {
    pub fn preview(&self) -> &SkillInstallationPreview {
        &self.preview
    }

    pub fn mutation(&self) -> &SkillInstallationMutation {
        &self.mutation
    }

    pub fn replayed(&self) -> bool {
        self.replayed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillPreparationCancellation {
    Cancelled,
    AlreadyCancelled,
}

#[derive(Clone)]
pub(super) enum PreparedAction {
    Install {
        installation_id: SkillInstallationId,
        skill_id: SkillId,
    },
    Update {
        installation_id: SkillInstallationId,
        skill_id: SkillId,
        expected_revision: SkillInstallationRevision,
    },
}

impl PreparedAction {
    pub(super) fn from_intent(
        intent: &SkillInstallationPreparationIntent,
    ) -> Result<Self, SkillInstallationWorkflowError> {
        let installed_source = SkillSourceId::parse(USER_INSTALLED_SKILL_SOURCE_ID)
            .expect("the built-in installed Skill source id must remain valid");
        match intent {
            SkillInstallationPreparationIntent::Install { installation_id } => {
                let skill_id = SkillId::from_parts(installed_source, installation_id.as_str())
                    .expect("a canonical installation id must form a valid Skill id");
                Ok(Self::Install {
                    installation_id: installation_id.clone(),
                    skill_id,
                })
            }
            SkillInstallationPreparationIntent::Update {
                skill_id,
                expected_revision,
            } => {
                if skill_id.source_id() != &installed_source {
                    return Err(SkillInstallationWorkflowError::InvalidUpdateTarget {
                        skill_id: skill_id.clone(),
                        reason: format!(
                            "Skill does not belong to the user-installed source `{installed_source}`",
                        ),
                    });
                }
                let installation_id =
                    SkillInstallationId::parse(skill_id.local_id()).map_err(|error| {
                        SkillInstallationWorkflowError::InvalidUpdateTarget {
                            skill_id: skill_id.clone(),
                            reason: format!("invalid installed Skill identity: {error}"),
                        }
                    })?;
                Ok(Self::Update {
                    installation_id,
                    skill_id: skill_id.clone(),
                    expected_revision: expected_revision.clone(),
                })
            }
        }
    }

    pub(super) fn operation(&self) -> SkillInstallationOperation {
        match self {
            Self::Install { .. } => SkillInstallationOperation::Install,
            Self::Update { .. } => SkillInstallationOperation::Update,
        }
    }

    pub(super) fn installation_id(&self) -> &SkillInstallationId {
        match self {
            Self::Install {
                installation_id, ..
            }
            | Self::Update {
                installation_id, ..
            } => installation_id,
        }
    }

    pub(super) fn skill_id(&self) -> &SkillId {
        match self {
            Self::Install { skill_id, .. } | Self::Update { skill_id, .. } => skill_id,
        }
    }

    pub(super) fn expected_revision(&self) -> Option<&SkillInstallationRevision> {
        match self {
            Self::Install { .. } => None,
            Self::Update {
                expected_revision, ..
            } => Some(expected_revision),
        }
    }
}

pub(super) fn build_preview(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    acquisition: &PreparedSkillAcquisition,
    current: Option<&InstalledSkillRecord>,
    expires_at_unix_ms: u64,
) -> SkillInstallationPreview {
    let package = acquisition.package();
    let index = package.resource_index();
    let mut resources = SkillPackageResourceSummary {
        resource_count: index.len(),
        ..SkillPackageResourceSummary::default()
    };
    for resource in index.entries() {
        resources.resource_bytes = resources
            .resource_bytes
            .saturating_add(resource.byte_length());
        match resource.kind() {
            SkillResourceKind::Reference => resources.reference_count += 1,
            SkillResourceKind::Asset => resources.asset_count += 1,
            SkillResourceKind::Script => resources.script_count += 1,
            SkillResourceKind::Other => {
                // Generic resources remain visible in the total count and byte
                // size; unlike scripts they do not introduce a special warning.
            }
        }
    }
    let mut warnings = Vec::new();
    if resources.resource_count > 0 {
        warnings.push(SkillInstallationWarning {
            code: SkillInstallationWarningCode::ResourcesNotExposed,
            message: "This version preserves sibling Skill files but does not yet expose them to the agent runtime. Instructions that depend on those files may not work."
                .to_string(),
            acknowledgement_required: false,
        });
    }
    if resources.script_count > 0 {
        warnings.push(SkillInstallationWarning {
            code: SkillInstallationWarningCode::ContainsScripts,
            message: "This Skill contains script files. They are installed as inert resources and are never executed automatically."
                .to_string(),
            acknowledgement_required: true,
        });
    }
    let package_preview = SkillInstallationPackagePreview {
        name: package.name().to_string(),
        description: package.description().to_string(),
        revision: package.revision().clone(),
        format_version: package.format_version(),
        entrypoint_bytes: u64::try_from(package.source_bytes().len()).unwrap_or(u64::MAX),
        resources,
    };
    let presentation = SkillAcquisitionPresentation {
        provider: package.origin().provider().to_string(),
        reference: package.origin().reference().to_string(),
    };
    let changes = current.map_or(
        PreviewChanges {
            content_changed: true,
            source_changed: true,
        },
        |record| PreviewChanges {
            content_changed: record.package_revision() != package.revision(),
            source_changed: record.provenance() != acquisition.provenance(),
        },
    );
    let preview_revision = preview_revision(
        request,
        action,
        &presentation,
        &package_preview,
        &warnings,
        changes,
        expires_at_unix_ms,
    );
    SkillInstallationPreview {
        preparation_id: request.preparation_id.clone(),
        preview_revision,
        operation: action.operation(),
        installation_id: action.installation_id().clone(),
        skill_id: action.skill_id().clone(),
        expected_revision: action.expected_revision().cloned(),
        content_changed: changes.content_changed,
        source_changed: changes.source_changed,
        acquisition: presentation,
        package: package_preview,
        warnings: warnings.into(),
        expires_at_unix_ms,
    }
}

#[derive(Clone, Copy)]
pub(super) struct PreviewChanges {
    pub(super) content_changed: bool,
    pub(super) source_changed: bool,
}

pub(super) fn preview_revision(
    request: &SkillInstallationPreparationRequest,
    action: &PreparedAction,
    acquisition: &SkillAcquisitionPresentation,
    package: &SkillInstallationPackagePreview,
    warnings: &[SkillInstallationWarning],
    changes: PreviewChanges,
    expires_at_unix_ms: u64,
) -> SkillPreviewRevision {
    let mut hasher = Sha256::new();
    update_hash_field(&mut hasher, b"mycopilot-skill-install-preview-v1");
    update_hash_field(&mut hasher, request.preparation_id.as_str().as_bytes());
    update_hash_field(&mut hasher, action.operation().stable_name().as_bytes());
    update_hash_field(&mut hasher, action.installation_id().as_str().as_bytes());
    update_hash_field(&mut hasher, action.skill_id().as_str().as_bytes());
    match action.expected_revision() {
        Some(revision) => {
            update_hash_field(&mut hasher, b"expected-revision-present");
            update_hash_field(&mut hasher, revision.as_str().as_bytes());
        }
        None => update_hash_field(&mut hasher, b"expected-revision-absent"),
    }
    update_hash_field(&mut hasher, request.source.provider().as_str().as_bytes());
    match &request.source {
        SkillAcquisitionSource::LocalDirectory { directory } => {
            update_hash_field(&mut hasher, b"local-directory");
            update_hash_field(&mut hasher, os_path_bytes(directory).as_ref());
        }
        SkillAcquisitionSource::Adapter { request, .. } => {
            update_hash_field(&mut hasher, b"adapter-request");
            update_hash_field(&mut hasher, request);
        }
        SkillAcquisitionSource::ResolvedCandidate {
            resolution_id,
            candidate_id,
        } => {
            update_hash_field(&mut hasher, b"resolved-candidate");
            update_hash_field(&mut hasher, resolution_id.as_str().as_bytes());
            update_hash_field(&mut hasher, candidate_id.as_str().as_bytes());
        }
        SkillAcquisitionSource::InstalledSource => {
            update_hash_field(&mut hasher, b"installed-source");
        }
    }
    update_hash_field(&mut hasher, acquisition.provider.as_bytes());
    update_hash_field(&mut hasher, acquisition.reference.as_bytes());
    update_hash_field(&mut hasher, package.revision.as_str().as_bytes());
    update_hash_field(&mut hasher, &package.format_version.to_be_bytes());
    update_hash_field(&mut hasher, package.name.as_bytes());
    update_hash_field(&mut hasher, package.description.as_bytes());
    update_hash_field(&mut hasher, &package.entrypoint_bytes.to_be_bytes());
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.resource_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(&mut hasher, &package.resources.resource_bytes.to_be_bytes());
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.reference_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.asset_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    update_hash_field(
        &mut hasher,
        &u64::try_from(package.resources.script_count)
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for warning in warnings {
        update_hash_field(&mut hasher, warning.code.stable_name().as_bytes());
        update_hash_field(&mut hasher, warning.message.as_bytes());
        update_hash_field(
            &mut hasher,
            if warning.acknowledgement_required {
                b"ack-required"
            } else {
                b"ack-not-required"
            },
        );
    }
    update_hash_field(
        &mut hasher,
        if changes.content_changed {
            b"content-changed"
        } else {
            b"content-unchanged"
        },
    );
    update_hash_field(
        &mut hasher,
        if changes.source_changed {
            b"source-changed"
        } else {
            b"source-unchanged"
        },
    );
    update_hash_field(&mut hasher, &expires_at_unix_ms.to_be_bytes());
    SkillPreviewRevision::from_digest(hasher.finalize().into())
}

pub(super) fn update_hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(unix)]
pub(super) fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(path.as_os_str().as_bytes())
}

#[cfg(windows)]
pub(super) fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    use std::os::windows::ffi::OsStrExt;
    let bytes = path
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    std::borrow::Cow::Owned(bytes)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn os_path_bytes(path: &Path) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Owned(path.to_string_lossy().into_owned().into_bytes())
}

pub(super) fn acquisition_snapshot_bytes(acquisition: &PreparedSkillAcquisition) -> usize {
    acquisition.retained_payload_bytes()
}

pub(super) fn invalid_acquisition_provider_output() -> SkillAcquisitionAdapterError {
    SkillAcquisitionAdapterError::unavailable(
        "the acquisition provider returned package or provenance metadata owned by a different provider",
    )
}

pub(super) fn ensure_preview_matches(
    preview: &SkillInstallationPreview,
    request: &SkillInstallationCommitRequest,
) -> Result<(), SkillInstallationWorkflowError> {
    if preview.preview_revision == request.expected_preview_revision {
        Ok(())
    } else {
        Err(SkillInstallationWorkflowError::PreviewMismatch {
            preparation_id: request.preparation_id.clone(),
            expected: request.expected_preview_revision.clone(),
            actual: preview.preview_revision.clone(),
        })
    }
}

pub(super) fn ensure_warnings_acknowledged(
    preview: &SkillInstallationPreview,
    request: &SkillInstallationCommitRequest,
) -> Result<(), SkillInstallationWorkflowError> {
    let missing = preview
        .warnings()
        .iter()
        .filter(|warning| {
            warning.acknowledgement_required()
                && !request.acknowledged_warnings.contains(&warning.code())
        })
        .map(SkillInstallationWarning::code)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(
            SkillInstallationWorkflowError::WarningAcknowledgementRequired {
                preparation_id: request.preparation_id.clone(),
                missing,
            },
        )
    }
}
