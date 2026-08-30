use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::run_grant::FileChangeDirectoryIdentity;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
#[cfg(not(unix))]
use std::fs;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const FILE_OBSERVATION_TTL_MS: u64 = 10 * 60 * 1_000;
pub const FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION: u32 = 2;
const FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION: u32 = 1;
const MAX_OBSERVATIONS_PER_RUN: usize = 1_024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct FileObservationOwner<'a> {
    source_tool_call_id: &'a str,
    conversation_id: &'a str,
    run_id: &'a str,
}

impl<'a> FileObservationOwner<'a> {
    pub(crate) fn new(
        source_tool_call_id: &'a str,
        conversation_id: &'a str,
        run_id: &'a str,
    ) -> Self {
        Self {
            source_tool_call_id,
            conversation_id,
            run_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileObservationIdentity {
    byte_count: u64,
    #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
    modified_ns: Option<u64>,
    #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
    device: Option<u64>,
    #[serde(deserialize_with = "crate::protocol::deserialize_required_nullable")]
    inode: Option<u64>,
}

impl FileObservationIdentity {
    pub fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            byte_count: metadata.len(),
            modified_ns: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_nanos().try_into().unwrap_or(u64::MAX)),
            #[cfg(unix)]
            device: Some(metadata.dev()),
            #[cfg(not(unix))]
            device: None,
            #[cfg(unix)]
            inode: Some(metadata.ino()),
            #[cfg(not(unix))]
            inode: None,
        }
    }

    fn validate(&self) -> FileChangeResultValue<()> {
        if self.device.is_some() != self.inode.is_some() {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(())
    }
}

/// Strict Observation-owned projection of a stable parent directory object identity.
///
/// Acquisition is shared with FileChange Run grants, but this private checkpoint DTO owns its
/// camelCase wire independently so changing it cannot reinterpret the existing Run-grant storage
/// shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FileObservationDirectoryIdentity {
    Unix {
        #[serde(deserialize_with = "deserialize_observation_directory_identity_schema_version")]
        schema_version: u32,
        device: u64,
        inode: u64,
    },
    Windows {
        #[serde(deserialize_with = "deserialize_observation_directory_identity_schema_version")]
        schema_version: u32,
        volume_serial_number: u64,
        file_id: String,
    },
}

fn deserialize_observation_directory_identity_schema_version<'de, D>(
    deserializer: D,
) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported FileObservation directory identity schema version",
        ))
    }
}

impl FileObservationDirectoryIdentity {
    pub fn read(path: &Path) -> Result<Self, &'static str> {
        Self::from_file_change_identity(FileChangeDirectoryIdentity::read(path)?)
    }

    #[cfg(unix)]
    fn from_bound_metadata(metadata: &Metadata) -> Result<Self, &'static str> {
        Self::from_file_change_identity(FileChangeDirectoryIdentity::from_bound_metadata(metadata)?)
    }

    fn from_file_change_identity(
        identity: FileChangeDirectoryIdentity,
    ) -> Result<Self, &'static str> {
        identity.validate()?;
        Ok(match identity {
            FileChangeDirectoryIdentity::Unix { device, inode, .. } => Self::Unix {
                schema_version: FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION,
                device,
                inode,
            },
            FileChangeDirectoryIdentity::Windows {
                volume_serial_number,
                file_id,
                ..
            } => Self::Windows {
                schema_version: FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION,
                volume_serial_number,
                file_id,
            },
        })
    }

    fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Unix { schema_version, .. }
                if *schema_version == FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION =>
            {
                Ok(())
            }
            Self::Windows {
                schema_version,
                file_id,
                ..
            } if *schema_version == FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION
                && file_id.len() == 32
                && file_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
            {
                Ok(())
            }
            _ => Err("invalid FileObservation parent directory identity"),
        }
    }
}

fn deserialize_observation_checkpoint_schema_version<'de, D>(
    deserializer: D,
) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported FileObservation checkpoint schema version",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileObservationState {
    Missing,
    Existing {
        revision: String,
        identity: FileObservationIdentity,
    },
}

/// Strict, provider-neutral durable projection of one unconsumed file observation.
///
/// This is checkpoint state rather than model wire. Every nullable identity field is still
/// required on the wire so a platform difference cannot be confused with an old shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileObservationCheckpoint {
    #[serde(deserialize_with = "deserialize_observation_checkpoint_schema_version")]
    pub schema_version: u32,
    pub observation_id: String,
    pub source_tool_call_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub canonical_target: String,
    pub state: FileObservationState,
    pub parent_directory_identity: FileObservationDirectoryIdentity,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
}

impl FileObservationCheckpoint {
    /// Validates a claimed observation embedded in a durable FileChange transaction.
    ///
    /// Expiry limits how long the observation may be *claimed*. Once a complete transaction has
    /// claimed it, approval may legitimately outlive that TTL, so this validation rejects future
    /// or malformed timestamps without treating an expired transaction as a blind write.
    pub fn validate_frozen_binding(
        &self,
        expected_conversation_id: &str,
        expected_run_id: &str,
        expected_target: &Path,
    ) -> Result<(), FileChangeError> {
        if self.schema_version != FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION
            || !valid_observation_id(&self.observation_id)
            || !valid_owned_id(&self.source_tool_call_id)
            || !valid_owned_id(&self.conversation_id)
            || !valid_owned_id(&self.run_id)
            || self.conversation_id != expected_conversation_id
            || self.run_id != expected_run_id
            || self.created_at_ms > now_ms()
            || self.expires_at_ms.checked_sub(self.created_at_ms) != Some(FILE_OBSERVATION_TTL_MS)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        self.parent_directory_identity.validate().map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::InvalidArguments, error)
        })?;
        if let FileObservationState::Existing { revision, identity } = &self.state {
            if !valid_owned_id(revision) {
                return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
            }
            identity.validate()?;
        }
        let canonical_target = Path::new(&self.canonical_target);
        if !canonical_target.is_absolute()
            || canonical_target != expected_target
            || self.canonical_target.trim() != self.canonical_target
            || self.canonical_target.chars().any(char::is_control)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(())
    }

    /// Rechecks the exact parent and leaf identity frozen by the Observation source immediately
    /// before a definitely-not-yet-executed transaction enters the committer.
    pub fn revalidate_current_identity(
        &self,
        expected_target: &Path,
    ) -> Result<(), FileChangeError> {
        self.revalidate_current_identity_with_hook(expected_target, || {})
    }

    fn revalidate_current_identity_with_hook(
        &self,
        expected_target: &Path,
        after_parent_bind: impl FnOnce(),
    ) -> Result<(), FileChangeError> {
        self.validate_frozen_binding(&self.conversation_id, &self.run_id, expected_target)?;

        #[cfg(unix)]
        {
            let parent = super::BoundReadParent::bind(expected_target).map_err(|error| {
                let code = if error.raw_os_error() == Some(libc::ELOOP) {
                    FileChangeErrorCode::SymlinkForbidden
                } else if error.kind() == std::io::ErrorKind::PermissionDenied {
                    FileChangeErrorCode::PermissionDenied
                } else {
                    FileChangeErrorCode::ObservationStale
                };
                FileChangeError::with_diagnostic(code, error.to_string())
            })?;
            after_parent_bind();
            let parent_metadata = parent.parent_metadata().map_err(|error| {
                FileChangeError::with_diagnostic(
                    FileChangeErrorCode::ObservationStale,
                    error.to_string(),
                )
            })?;
            if self.parent_directory_identity
                != parent_directory_identity(expected_target, &parent_metadata)?
            {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            let current = parent.inspect_optional_leaf()?;
            match (&self.state, current) {
                (FileObservationState::Missing, None) => Ok(()),
                (FileObservationState::Missing, Some(_)) => {
                    Err(FileChangeError::new(FileChangeErrorCode::FileExists))
                }
                (FileObservationState::Existing { .. }, None) => {
                    Err(FileChangeError::new(FileChangeErrorCode::FileMissing))
                }
                (FileObservationState::Existing { identity, .. }, Some(metadata))
                    if *identity == FileObservationIdentity::from_metadata(&metadata) =>
                {
                    Ok(())
                }
                (FileObservationState::Existing { .. }, Some(_)) => {
                    Err(FileChangeError::new(FileChangeErrorCode::ObservationStale))
                }
            }
        }

        #[cfg(not(unix))]
        {
            after_parent_bind();
            let parent = expected_target
                .parent()
                .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ParentMissing))?;
            let parent_metadata = fs::metadata(parent).map_err(|error| {
                FileChangeError::with_diagnostic(
                    FileChangeErrorCode::ObservationStale,
                    error.to_string(),
                )
            })?;
            if self.parent_directory_identity
                != parent_directory_identity(expected_target, &parent_metadata)?
            {
                return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
            }
            match &self.state {
                FileObservationState::Missing => match fs::symlink_metadata(expected_target) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Ok(_) => Err(FileChangeError::new(FileChangeErrorCode::FileExists)),
                    Err(error) => Err(FileChangeError::with_diagnostic(
                        FileChangeErrorCode::ObservationStale,
                        error.to_string(),
                    )),
                },
                FileObservationState::Existing { identity, .. } => {
                    let metadata = fs::symlink_metadata(expected_target).map_err(|error| {
                        let code = if error.kind() == std::io::ErrorKind::NotFound {
                            FileChangeErrorCode::FileMissing
                        } else {
                            FileChangeErrorCode::ObservationStale
                        };
                        FileChangeError::with_diagnostic(code, error.to_string())
                    })?;
                    if metadata.file_type().is_symlink()
                        || !metadata.is_file()
                        || *identity != FileObservationIdentity::from_metadata(&metadata)
                    {
                        return Err(FileChangeError::new(FileChangeErrorCode::ObservationStale));
                    }
                    Ok(())
                }
            }
        }
    }
}

impl FileObservationState {
    pub fn revision(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Existing { revision, .. } => Some(revision),
        }
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileObservation {
    id: String,
    source_tool_call_id: String,
    conversation_id: String,
    run_id: String,
    canonical_target: PathBuf,
    state: FileObservationState,
    parent_directory_identity: FileObservationDirectoryIdentity,
    created_at: u64,
    expires_at: u64,
}

impl FileObservation {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn canonical_target(&self) -> &Path {
        &self.canonical_target
    }

    pub fn source_tool_call_id(&self) -> &str {
        &self.source_tool_call_id
    }

    pub fn state(&self) -> &FileObservationState {
        &self.state
    }

    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn parent_identity_matches(
        &self,
        target: &Path,
        metadata: &Metadata,
    ) -> FileChangeResultValue<bool> {
        Ok(self.parent_directory_identity == parent_directory_identity(target, metadata)?)
    }

    pub(crate) fn checkpoint(&self) -> FileObservationCheckpoint {
        FileObservationCheckpoint {
            schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            observation_id: self.id.clone(),
            source_tool_call_id: self.source_tool_call_id.clone(),
            conversation_id: self.conversation_id.clone(),
            run_id: self.run_id.clone(),
            canonical_target: self.canonical_target.to_string_lossy().into_owned(),
            state: self.state.clone(),
            parent_directory_identity: self.parent_directory_identity.clone(),
            created_at_ms: self.created_at,
            expires_at_ms: self.expires_at,
        }
    }

    fn from_checkpoint(
        checkpoint: FileObservationCheckpoint,
        expected_conversation_id: &str,
        expected_run_id: &str,
        now: u64,
    ) -> FileChangeResultValue<Self> {
        let canonical_target = PathBuf::from(&checkpoint.canonical_target);
        checkpoint.validate_frozen_binding(
            expected_conversation_id,
            expected_run_id,
            &canonical_target,
        )?;
        if checkpoint.created_at_ms > now || checkpoint.expires_at_ms <= now {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(Self {
            id: checkpoint.observation_id,
            source_tool_call_id: checkpoint.source_tool_call_id,
            conversation_id: checkpoint.conversation_id,
            run_id: checkpoint.run_id,
            canonical_target,
            state: checkpoint.state,
            parent_directory_identity: checkpoint.parent_directory_identity,
            created_at: checkpoint.created_at_ms,
            expires_at: checkpoint.expires_at_ms,
        })
    }
}

#[derive(Debug, Default)]
pub struct FileObservationRegistry {
    observations: Mutex<HashMap<String, FileObservation>>,
}

impl FileObservationRegistry {
    pub(crate) fn issue_existing(
        &self,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        revision: &str,
        metadata: &Metadata,
        parent_metadata: &Metadata,
    ) -> FileChangeResultValue<FileObservation> {
        self.issue(
            owner,
            canonical_target,
            FileObservationState::Existing {
                revision: revision.to_string(),
                identity: FileObservationIdentity::from_metadata(metadata),
            },
            parent_metadata,
            now_ms(),
        )
    }

    pub(crate) fn issue_missing(
        &self,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        parent_metadata: &Metadata,
    ) -> FileChangeResultValue<FileObservation> {
        self.issue(
            owner,
            canonical_target,
            FileObservationState::Missing,
            parent_metadata,
            now_ms(),
        )
    }

    /// Captures a Host-authenticated Missing binding without registering model authority.
    ///
    /// This is the create counterpart to an observation supplied by `read_file`: proposal and
    /// staged-begin code can freeze the exact parent/leaf state in a private checkpoint without
    /// first handing the model a token for a file that does not exist. Because the returned value
    /// is deliberately absent from `observations`, it cannot be validated or claimed as live
    /// authority. Its private id remains execution-only; a verified successful create issues the
    /// first model-visible Observation separately.
    pub(crate) fn capture_missing(
        &self,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        parent_metadata: &Metadata,
    ) -> FileChangeResultValue<FileObservation> {
        self.capture_missing_at(owner, canonical_target, parent_metadata, now_ms())
    }

    /// Reissues a consumed predecessor id for a verified post-write existing file.
    ///
    /// The predecessor must have been atomically removed by `claim`. An occupied id, or another
    /// live observation issued by the same source call, fails closed. Repeating the exact same
    /// issuance is idempotent and returns the already registered successor.
    pub(crate) fn reissue_existing(
        &self,
        predecessor_observation_id: &str,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        revision: &str,
        metadata: &Metadata,
        parent_metadata: &Metadata,
    ) -> FileChangeResultValue<FileObservation> {
        self.reissue(
            predecessor_observation_id,
            owner,
            canonical_target,
            FileObservationState::Existing {
                revision: revision.to_string(),
                identity: FileObservationIdentity::from_metadata(metadata),
            },
            parent_metadata,
            now_ms(),
        )
    }

    /// Reissues a consumed predecessor id for a verified post-write missing file.
    pub(crate) fn reissue_missing(
        &self,
        predecessor_observation_id: &str,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        parent_metadata: &Metadata,
    ) -> FileChangeResultValue<FileObservation> {
        self.reissue(
            predecessor_observation_id,
            owner,
            canonical_target,
            FileObservationState::Missing,
            parent_metadata,
            now_ms(),
        )
    }

    pub fn validate(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
    ) -> FileChangeResultValue<FileObservation> {
        self.validate_at(
            observation_id,
            conversation_id,
            run_id,
            canonical_target,
            now_ms(),
        )
    }

    /// Atomically claims an observation after a Direct proposal has been fully validated.
    ///
    /// Callers must not claim before proposal construction succeeds: malformed edits are allowed
    /// to be corrected while the same current observation remains valid. A successful claim makes
    /// replay with the same observation fail closed.
    pub(crate) fn claim(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
        consumer_tool_call_id: &str,
    ) -> FileChangeResultValue<FileObservation> {
        if !valid_owned_id(consumer_tool_call_id) {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        self.claim_at(
            observation_id,
            conversation_id,
            run_id,
            canonical_target,
            consumer_tool_call_id,
            now_ms(),
        )
    }

    pub(crate) fn checkpoint_exact(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
    ) -> FileChangeResultValue<FileObservationCheckpoint> {
        Ok(self
            .validate_at(
                observation_id,
                conversation_id,
                run_id,
                canonical_target,
                now_ms(),
            )?
            .checkpoint())
    }

    pub(crate) fn from_checkpoints(
        checkpoints: Vec<FileObservationCheckpoint>,
        expected_conversation_id: &str,
        expected_run_id: &str,
    ) -> FileChangeResultValue<Self> {
        Self::from_checkpoints_at(
            checkpoints,
            expected_conversation_id,
            expected_run_id,
            now_ms(),
        )
    }

    fn issue(
        &self,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        state: FileObservationState,
        parent_metadata: &Metadata,
        now: u64,
    ) -> FileChangeResultValue<FileObservation> {
        if !valid_owned_id(owner.source_tool_call_id)
            || !valid_owned_id(owner.conversation_id)
            || !valid_owned_id(owner.run_id)
            || !canonical_target.is_absolute()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let parent_directory_identity =
            parent_directory_identity(canonical_target, parent_metadata)?;
        let expires_at = now.saturating_add(FILE_OBSERVATION_TTL_MS);
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        observations.retain(|_, observation| observation.expires_at > now);
        if let Some(existing) = observations
            .values()
            .find(|observation| observation.source_tool_call_id == owner.source_tool_call_id)
        {
            if existing.conversation_id == owner.conversation_id
                && existing.run_id == owner.run_id
                && existing.canonical_target == canonical_target
                && existing.state == state
                && existing.parent_directory_identity == parent_directory_identity
            {
                return Ok(existing.clone());
            }
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        if observations.len() >= MAX_OBSERVATIONS_PER_RUN {
            let oldest = observations
                .values()
                .min_by_key(|observation| observation.created_at)
                .map(|observation| observation.id.clone());
            if let Some(oldest) = oldest {
                observations.remove(&oldest);
            }
        }
        let observation = FileObservation {
            id: format!("fobs_{}", Uuid::new_v4().simple()),
            source_tool_call_id: owner.source_tool_call_id.to_string(),
            conversation_id: owner.conversation_id.to_string(),
            run_id: owner.run_id.to_string(),
            canonical_target: canonical_target.to_path_buf(),
            state,
            parent_directory_identity,
            created_at: now,
            expires_at,
        };
        observations.insert(observation.id.clone(), observation.clone());
        Ok(observation)
    }

    fn capture_missing_at(
        &self,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        parent_metadata: &Metadata,
        now: u64,
    ) -> FileChangeResultValue<FileObservation> {
        if !valid_owned_id(owner.source_tool_call_id)
            || !valid_owned_id(owner.conversation_id)
            || !valid_owned_id(owner.run_id)
            || !canonical_target.is_absolute()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(FileObservation {
            id: format!("fobs_{}", Uuid::new_v4().simple()),
            source_tool_call_id: owner.source_tool_call_id.to_string(),
            conversation_id: owner.conversation_id.to_string(),
            run_id: owner.run_id.to_string(),
            canonical_target: canonical_target.to_path_buf(),
            state: FileObservationState::Missing,
            parent_directory_identity: parent_directory_identity(
                canonical_target,
                parent_metadata,
            )?,
            created_at: now,
            expires_at: now.saturating_add(FILE_OBSERVATION_TTL_MS),
        })
    }

    fn reissue(
        &self,
        predecessor_observation_id: &str,
        owner: FileObservationOwner<'_>,
        canonical_target: &Path,
        state: FileObservationState,
        parent_metadata: &Metadata,
        now: u64,
    ) -> FileChangeResultValue<FileObservation> {
        if !valid_observation_id(predecessor_observation_id)
            || !valid_owned_id(owner.source_tool_call_id)
            || !valid_owned_id(owner.conversation_id)
            || !valid_owned_id(owner.run_id)
            || !canonical_target.is_absolute()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        if let FileObservationState::Existing { revision, identity } = &state {
            if !valid_owned_id(revision) {
                return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
            }
            identity.validate()?;
        }
        let parent_directory_identity =
            parent_directory_identity(canonical_target, parent_metadata)?;
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        observations.retain(|_, observation| observation.expires_at > now);

        if let Some(existing) = observations.get(predecessor_observation_id) {
            if existing.source_tool_call_id == owner.source_tool_call_id
                && existing.conversation_id == owner.conversation_id
                && existing.run_id == owner.run_id
                && existing.canonical_target == canonical_target
                && existing.state == state
                && existing.parent_directory_identity == parent_directory_identity
            {
                return Ok(existing.clone());
            }
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        if observations
            .values()
            .any(|observation| observation.source_tool_call_id == owner.source_tool_call_id)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        if observations.len() >= MAX_OBSERVATIONS_PER_RUN {
            let oldest = observations
                .values()
                .min_by_key(|observation| observation.created_at)
                .map(|observation| observation.id.clone());
            if let Some(oldest) = oldest {
                observations.remove(&oldest);
            }
        }
        let observation = FileObservation {
            id: predecessor_observation_id.to_string(),
            source_tool_call_id: owner.source_tool_call_id.to_string(),
            conversation_id: owner.conversation_id.to_string(),
            run_id: owner.run_id.to_string(),
            canonical_target: canonical_target.to_path_buf(),
            state,
            parent_directory_identity,
            created_at: now,
            expires_at: now.saturating_add(FILE_OBSERVATION_TTL_MS),
        };
        observations.insert(observation.id.clone(), observation.clone());
        Ok(observation)
    }

    fn validate_at(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
        now: u64,
    ) -> FileChangeResultValue<FileObservation> {
        if observation_id.trim().is_empty() {
            return Err(FileChangeError::new(
                FileChangeErrorCode::ObservationRequired,
            ));
        }
        let observations = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let observation = observations
            .get(observation_id)
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ObservationRequired))?;
        if observation.expires_at <= now {
            return Err(FileChangeError::new(
                FileChangeErrorCode::ObservationExpired,
            ));
        }
        if observation.conversation_id != conversation_id || observation.run_id != run_id {
            return Err(FileChangeError::new(
                FileChangeErrorCode::ObservationOwnerMismatch,
            ));
        }
        if observation.canonical_target != canonical_target {
            return Err(FileChangeError::new(
                FileChangeErrorCode::ObservationPathMismatch,
            ));
        }
        Ok(observation.clone())
    }

    fn claim_at(
        &self,
        observation_id: &str,
        conversation_id: &str,
        run_id: &str,
        canonical_target: &Path,
        consumer_tool_call_id: &str,
        now: u64,
    ) -> FileChangeResultValue<FileObservation> {
        let mut observations = self
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let observation = observations
            .get(observation_id)
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ObservationRequired))?;
        validate_observation_binding(observation, conversation_id, run_id, canonical_target, now)?;
        if observation.source_tool_call_id == consumer_tool_call_id {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(observations
            .remove(observation_id)
            .expect("validated observation remains under the registry lock"))
    }

    fn from_checkpoints_at(
        checkpoints: Vec<FileObservationCheckpoint>,
        expected_conversation_id: &str,
        expected_run_id: &str,
        now: u64,
    ) -> FileChangeResultValue<Self> {
        if checkpoints.len() > MAX_OBSERVATIONS_PER_RUN
            || !valid_owned_id(expected_conversation_id)
            || !valid_owned_id(expected_run_id)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let mut observations = HashMap::with_capacity(checkpoints.len());
        let mut source_call_ids = HashSet::with_capacity(checkpoints.len());
        for checkpoint in checkpoints {
            let observation = FileObservation::from_checkpoint(
                checkpoint,
                expected_conversation_id,
                expected_run_id,
                now,
            )?;
            if !source_call_ids.insert(observation.source_tool_call_id.clone())
                || observations
                    .insert(observation.id.clone(), observation)
                    .is_some()
            {
                return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
            }
        }
        Ok(Self {
            observations: Mutex::new(observations),
        })
    }
}

fn parent_directory_identity(
    canonical_target: &Path,
    parent_metadata: &Metadata,
) -> FileChangeResultValue<FileObservationDirectoryIdentity> {
    let parent = canonical_target
        .parent()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::ParentMissing))?;

    #[cfg(unix)]
    {
        let _ = parent;
        FileObservationDirectoryIdentity::from_bound_metadata(parent_metadata).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::ObservationStale, error)
        })
    }

    #[cfg(windows)]
    {
        let _ = parent_metadata;
        FileObservationDirectoryIdentity::read(parent).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::ObservationStale, error)
        })
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (parent, parent_metadata);
        Err(FileChangeError::with_diagnostic(
            FileChangeErrorCode::ObservationStale,
            "stable parent directory identity is unsupported on this platform",
        ))
    }
}

fn validate_observation_binding(
    observation: &FileObservation,
    conversation_id: &str,
    run_id: &str,
    canonical_target: &Path,
    now: u64,
) -> FileChangeResultValue<()> {
    if observation.expires_at <= now {
        return Err(FileChangeError::new(
            FileChangeErrorCode::ObservationExpired,
        ));
    }
    if observation.conversation_id != conversation_id || observation.run_id != run_id {
        return Err(FileChangeError::new(
            FileChangeErrorCode::ObservationOwnerMismatch,
        ));
    }
    if observation.canonical_target != canonical_target {
        return Err(FileChangeError::new(
            FileChangeErrorCode::ObservationPathMismatch,
        ));
    }
    Ok(())
}

fn valid_observation_id(value: &str) -> bool {
    value.len() == 37
        && value.starts_with("fobs_")
        && value[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_owned_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2_048
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn every_source_call_gets_a_fresh_owner_path_and_time_bound_observation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.txt");
        fs::write(&path, "hello\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let first = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &path,
                FileObservationState::Existing {
                    revision: "revision-1".to_string(),
                    identity: FileObservationIdentity::from_metadata(&metadata),
                },
                &parent,
                100,
            )
            .unwrap();
        let second = registry
            .issue(
                FileObservationOwner::new("read-call-2", "conversation-1", "run-1"),
                &path,
                first.state.clone(),
                &parent,
                101,
            )
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.source_tool_call_id(), "read-call-1");
        assert_eq!(second.source_tool_call_id(), "read-call-2");
        assert!(registry
            .validate_at(&first.id, "conversation-1", "run-1", &path, 102)
            .is_ok());
        assert_eq!(
            registry
                .validate_at(&first.id, "conversation-2", "run-1", &path, 102)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationOwnerMismatch
        );
        assert_eq!(
            registry
                .validate_at(&first.id, "conversation-1", "run-2", &path, 102)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationOwnerMismatch
        );
        assert_eq!(
            registry
                .validate_at(
                    &first.id,
                    "conversation-1",
                    "run-1",
                    &directory.path().join("other.txt"),
                    102,
                )
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationPathMismatch
        );
        assert_eq!(
            registry
                .validate_at(
                    &first.id,
                    "conversation-1",
                    "run-1",
                    &path,
                    100 + FILE_OBSERVATION_TTL_MS,
                )
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationExpired
        );
    }

    #[test]
    fn repeated_issuance_for_the_same_source_is_idempotent_and_cannot_change_binding() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.txt");
        let second_path = directory.path().join("second.txt");
        fs::write(&first_path, "hello\n").unwrap();
        fs::write(&second_path, "other\n").unwrap();
        let first_metadata = fs::metadata(&first_path).unwrap();
        let second_metadata = fs::metadata(&second_path).unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let owner = FileObservationOwner::new("apply-call-1", "conversation-1", "run-1");
        let state = FileObservationState::Existing {
            revision: "revision-1".to_string(),
            identity: FileObservationIdentity::from_metadata(&first_metadata),
        };

        let first = registry
            .issue(owner, &first_path, state.clone(), &parent, 100)
            .unwrap();
        let replay = registry
            .issue(owner, &first_path, state, &parent, 101)
            .unwrap();
        assert_eq!(replay, first);

        let changed_target = registry
            .issue(
                owner,
                &second_path,
                FileObservationState::Existing {
                    revision: "revision-2".to_string(),
                    identity: FileObservationIdentity::from_metadata(&second_metadata),
                },
                &parent,
                102,
            )
            .unwrap_err();
        assert_eq!(changed_target.code(), FileChangeErrorCode::InvalidArguments);
    }

    #[test]
    fn claimed_observation_cannot_be_replayed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.txt");
        fs::write(&path, "hello\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &path,
                FileObservationState::Existing {
                    revision: "revision-1".to_string(),
                    identity: FileObservationIdentity::from_metadata(&metadata),
                },
                &parent,
                100,
            )
            .unwrap();

        let claimed = registry
            .claim_at(
                observation.id(),
                "conversation-1",
                "run-1",
                &path,
                "apply-call-1",
                101,
            )
            .unwrap();
        assert_eq!(claimed, observation);
        assert_eq!(
            registry
                .validate_at(observation.id(), "conversation-1", "run-1", &path, 102,)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationRequired
        );
    }

    #[test]
    fn captured_missing_binding_is_checkpointable_but_not_registered_authority() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("new.txt");
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();

        let captured = registry
            .capture_missing_at(
                FileObservationOwner::new("create-call-1", "conversation-1", "run-1"),
                &path,
                &parent,
                100,
            )
            .unwrap();

        assert!(valid_observation_id(captured.id()));
        assert_eq!(captured.source_tool_call_id(), "create-call-1");
        assert!(captured.state().is_missing());
        assert_eq!(captured.created_at(), 100);
        assert_eq!(captured.expires_at(), 100 + FILE_OBSERVATION_TTL_MS);
        captured
            .checkpoint()
            .validate_frozen_binding("conversation-1", "run-1", &path)
            .unwrap();
        assert_eq!(
            registry
                .validate_at(captured.id(), "conversation-1", "run-1", &path, 101)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationRequired
        );
        assert_eq!(
            registry
                .claim_at(
                    captured.id(),
                    "conversation-1",
                    "run-1",
                    &path,
                    "consumer-call-1",
                    101,
                )
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationRequired
        );
        assert!(registry
            .observations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
    }

    #[test]
    fn reissue_preserves_predecessor_id_and_refreshes_owner_state_and_ttl() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.txt");
        fs::write(&path, "first\n").unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let predecessor = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &path,
                FileObservationState::Existing {
                    revision: "revision-1".to_string(),
                    identity: FileObservationIdentity::from_metadata(&fs::metadata(&path).unwrap()),
                },
                &parent,
                100,
            )
            .unwrap();
        registry
            .claim_at(
                predecessor.id(),
                "conversation-1",
                "run-1",
                &path,
                "apply-call-1",
                101,
            )
            .unwrap();

        fs::write(&path, "second version\n").unwrap();
        let existing_state = FileObservationState::Existing {
            revision: "revision-2".to_string(),
            identity: FileObservationIdentity::from_metadata(&fs::metadata(&path).unwrap()),
        };
        let successor = registry
            .reissue(
                predecessor.id(),
                FileObservationOwner::new("apply-call-1", "conversation-1", "run-1"),
                &path,
                existing_state.clone(),
                &parent,
                200,
            )
            .unwrap();

        assert_eq!(successor.id(), predecessor.id());
        assert_eq!(successor.source_tool_call_id(), "apply-call-1");
        assert_eq!(successor.state(), &existing_state);
        assert_eq!(successor.created_at(), 200);
        assert_eq!(successor.expires_at(), 200 + FILE_OBSERVATION_TTL_MS);
        let exact_replay = registry
            .reissue(
                predecessor.id(),
                FileObservationOwner::new("apply-call-1", "conversation-1", "run-1"),
                &path,
                existing_state,
                &parent,
                201,
            )
            .unwrap();
        assert_eq!(exact_replay, successor);

        registry
            .claim_at(
                successor.id(),
                "conversation-1",
                "run-1",
                &path,
                "apply-call-2",
                202,
            )
            .unwrap();
        fs::remove_file(&path).unwrap();
        let deleted = registry
            .reissue(
                successor.id(),
                FileObservationOwner::new("apply-call-2", "conversation-1", "run-1"),
                &path,
                FileObservationState::Missing,
                &parent,
                300,
            )
            .unwrap();
        assert_eq!(deleted.id(), predecessor.id());
        assert_eq!(deleted.source_tool_call_id(), "apply-call-2");
        assert!(deleted.state().is_missing());
        assert_eq!(deleted.created_at(), 300);
        assert_eq!(deleted.expires_at(), 300 + FILE_OBSERVATION_TTL_MS);
    }

    #[test]
    fn reissue_fails_closed_for_an_occupied_predecessor_or_source_conflict() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.txt");
        let other_path = directory.path().join("other.txt");
        fs::write(&path, "first\n").unwrap();
        fs::write(&other_path, "other\n").unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let state = FileObservationState::Existing {
            revision: "revision-1".to_string(),
            identity: FileObservationIdentity::from_metadata(&fs::metadata(&path).unwrap()),
        };
        let predecessor = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &path,
                state.clone(),
                &parent,
                100,
            )
            .unwrap();

        let occupied = registry
            .reissue(
                predecessor.id(),
                FileObservationOwner::new("apply-call-1", "conversation-1", "run-1"),
                &path,
                state.clone(),
                &parent,
                101,
            )
            .unwrap_err();
        assert_eq!(occupied.code(), FileChangeErrorCode::InvalidArguments);

        registry
            .claim_at(
                predecessor.id(),
                "conversation-1",
                "run-1",
                &path,
                "proposal-call-1",
                102,
            )
            .unwrap();
        registry
            .issue(
                FileObservationOwner::new("apply-call-1", "conversation-1", "run-1"),
                &other_path,
                FileObservationState::Existing {
                    revision: "revision-other".to_string(),
                    identity: FileObservationIdentity::from_metadata(
                        &fs::metadata(&other_path).unwrap(),
                    ),
                },
                &parent,
                103,
            )
            .unwrap();
        let source_conflict = registry
            .reissue(
                predecessor.id(),
                FileObservationOwner::new("apply-call-1", "conversation-1", "run-1"),
                &path,
                state,
                &parent,
                104,
            )
            .unwrap_err();
        assert_eq!(
            source_conflict.code(),
            FileChangeErrorCode::InvalidArguments
        );
    }

    #[test]
    fn checkpoint_restore_rejects_schema_owner_path_expiry_and_duplicates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.txt");
        fs::write(&path, "hello\n").unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let parent = fs::metadata(directory.path()).unwrap();
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &path,
                FileObservationState::Existing {
                    revision: "revision-1".to_string(),
                    identity: FileObservationIdentity::from_metadata(&metadata),
                },
                &parent,
                100,
            )
            .unwrap();
        let checkpoint = observation.checkpoint();

        let restored = FileObservationRegistry::from_checkpoints_at(
            vec![checkpoint.clone()],
            "conversation-1",
            "run-1",
            101,
        )
        .unwrap();
        assert!(restored
            .validate_at(observation.id(), "conversation-1", "run-1", &path, 102,)
            .is_ok());

        let mut malformed = checkpoint.clone();
        malformed.schema_version += 1;
        assert!(FileObservationRegistry::from_checkpoints_at(
            vec![malformed],
            "conversation-1",
            "run-1",
            101,
        )
        .is_err());
        let mut wrong_path = checkpoint.clone();
        wrong_path.canonical_target = directory.path().join("other.txt").display().to_string();
        let restored_wrong_path = FileObservationRegistry::from_checkpoints_at(
            vec![wrong_path],
            "conversation-1",
            "run-1",
            101,
        )
        .unwrap();
        assert_eq!(
            restored_wrong_path
                .validate_at(observation.id(), "conversation-1", "run-1", &path, 102,)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationPathMismatch
        );
        assert!(FileObservationRegistry::from_checkpoints_at(
            vec![checkpoint.clone()],
            "conversation-2",
            "run-1",
            101,
        )
        .is_err());
        assert!(FileObservationRegistry::from_checkpoints_at(
            vec![checkpoint.clone()],
            "conversation-1",
            "run-1",
            checkpoint.expires_at_ms,
        )
        .is_err());
        assert!(FileObservationRegistry::from_checkpoints_at(
            vec![checkpoint.clone(), checkpoint],
            "conversation-1",
            "run-1",
            101,
        )
        .is_err());
    }

    #[test]
    fn frozen_binding_accepts_elapsed_ttl_but_rejects_a_tampered_interval() {
        let now = now_ms();
        let created_at_ms = now
            .saturating_sub(FILE_OBSERVATION_TTL_MS)
            .saturating_sub(1);
        let directory = tempfile::tempdir().unwrap();
        let directory_path = directory.path().canonicalize().unwrap();
        let target = directory_path.join("example.txt");
        let checkpoint = FileObservationCheckpoint {
            schema_version: FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            observation_id: "fobs_0123456789abcdef0123456789abcdef".to_string(),
            source_tool_call_id: "read-call-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            run_id: "run-1".to_string(),
            canonical_target: target.to_string_lossy().into_owned(),
            state: FileObservationState::Missing,
            parent_directory_identity: parent_directory_identity(
                &target,
                &fs::metadata(&directory_path).unwrap(),
            )
            .unwrap(),
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(FILE_OBSERVATION_TTL_MS),
        };
        checkpoint
            .validate_frozen_binding("conversation-1", "run-1", &target)
            .expect("a claimed transaction may outlive the observation claim TTL");

        let mut tampered = checkpoint;
        tampered.expires_at_ms = tampered.expires_at_ms.saturating_add(1);
        assert_eq!(
            tampered
                .validate_frozen_binding("conversation-1", "run-1", &target)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::InvalidArguments
        );
    }

    #[test]
    fn checkpoint_revalidation_allows_an_unrelated_sibling_create() {
        let directory = tempfile::tempdir().unwrap();
        let directory_path = directory.path().canonicalize().unwrap();
        let target = directory_path.join("target.txt");
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &target,
                FileObservationState::Missing,
                &fs::metadata(&directory_path).unwrap(),
                now_ms().saturating_sub(1),
            )
            .unwrap()
            .checkpoint();

        fs::write(directory_path.join("sibling.txt"), "sibling\n").unwrap();

        observation
            .revalidate_current_identity(&target)
            .expect("a sibling entry does not replace the observed parent or target leaf");
    }

    #[test]
    fn checkpoint_revalidation_rejects_creation_of_the_exact_missing_leaf() {
        let directory = tempfile::tempdir().unwrap();
        let directory_path = directory.path().canonicalize().unwrap();
        let target = directory_path.join("target.txt");
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &target,
                FileObservationState::Missing,
                &fs::metadata(&directory_path).unwrap(),
                now_ms().saturating_sub(1),
            )
            .unwrap()
            .checkpoint();

        fs::write(&target, "created externally\n").unwrap();

        assert_eq!(
            observation
                .revalidate_current_identity(&target)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::FileExists
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "created externally\n");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn checkpoint_revalidation_rejects_a_replaced_parent_directory() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let parent = root_path.join("parent");
        let displaced = root_path.join("displaced");
        fs::create_dir(&parent).unwrap();
        let target = parent.join("target.txt");
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &target,
                FileObservationState::Missing,
                &fs::metadata(&parent).unwrap(),
                now_ms().saturating_sub(1),
            )
            .unwrap()
            .checkpoint();

        fs::rename(&parent, &displaced).unwrap();
        fs::create_dir(&parent).unwrap();

        assert_eq!(
            observation
                .revalidate_current_identity(&target)
                .unwrap_err()
                .code(),
            FileChangeErrorCode::ObservationStale
        );
    }

    #[cfg(unix)]
    #[test]
    fn checkpoint_revalidation_cannot_follow_a_swapped_ancestor_or_leaf() {
        let ancestor_root = tempfile::tempdir().unwrap();
        let original_parent = ancestor_root.path().join("parent");
        let displaced_parent = ancestor_root.path().join("displaced");
        let outside_parent = ancestor_root.path().join("outside");
        fs::create_dir(&original_parent).unwrap();
        fs::create_dir(&outside_parent).unwrap();
        let target = original_parent.join("target.txt");
        fs::write(&target, "authorized\n").unwrap();
        fs::write(outside_parent.join("target.txt"), "must not be inspected\n").unwrap();
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-1", "conversation-1", "run-1"),
                &target,
                FileObservationState::Existing {
                    revision: "revision-1".to_string(),
                    identity: FileObservationIdentity::from_metadata(
                        &fs::metadata(&target).unwrap(),
                    ),
                },
                &fs::metadata(&original_parent).unwrap(),
                now_ms().saturating_sub(1),
            )
            .unwrap()
            .checkpoint();
        let error = observation
            .revalidate_current_identity_with_hook(&target, || {
                fs::rename(&original_parent, &displaced_parent).unwrap();
                symlink(&outside_parent, &original_parent).unwrap();
            })
            .unwrap_err();
        assert!(matches!(
            error.code(),
            FileChangeErrorCode::SymlinkForbidden | FileChangeErrorCode::ObservationStale
        ));

        let leaf_root = tempfile::tempdir().unwrap();
        let leaf_target = leaf_root.path().join("target.txt");
        let outside = leaf_root.path().join("outside.txt");
        fs::write(&leaf_target, "authorized\n").unwrap();
        fs::write(&outside, "must not be inspected\n").unwrap();
        let registry = FileObservationRegistry::default();
        let observation = registry
            .issue(
                FileObservationOwner::new("read-call-2", "conversation-1", "run-1"),
                &leaf_target,
                FileObservationState::Existing {
                    revision: "revision-2".to_string(),
                    identity: FileObservationIdentity::from_metadata(
                        &fs::metadata(&leaf_target).unwrap(),
                    ),
                },
                &fs::metadata(leaf_root.path()).unwrap(),
                now_ms().saturating_sub(1),
            )
            .unwrap()
            .checkpoint();
        let error = observation
            .revalidate_current_identity_with_hook(&leaf_target, || {
                fs::remove_file(&leaf_target).unwrap();
                symlink(&outside, &leaf_target).unwrap();
            })
            .unwrap_err();
        assert_eq!(error.code(), FileChangeErrorCode::SymlinkForbidden);
    }

    #[test]
    fn checkpoint_json_is_strict_and_rejects_the_previous_parent_identity_shape() {
        let checkpoint = serde_json::json!({
            "schemaVersion": FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION,
            "observationId": "fobs_0123456789abcdef0123456789abcdef",
            "sourceToolCallId": "read-call-1",
            "conversationId": "conversation-1",
            "runId": "run-1",
            "canonicalTarget": "/tmp/example.txt",
            "state": { "kind": "missing" },
            "parentDirectoryIdentity": {
                "kind": "unix",
                "schemaVersion": FILE_OBSERVATION_DIRECTORY_IDENTITY_SCHEMA_VERSION,
                "device": 1,
                "inode": 2
            },
            "createdAtMs": 100,
            "expiresAtMs": 100 + FILE_OBSERVATION_TTL_MS
        });
        serde_json::from_value::<FileObservationCheckpoint>(checkpoint.clone()).unwrap();

        let mut missing = checkpoint.clone();
        missing["parentDirectoryIdentity"]
            .as_object_mut()
            .unwrap()
            .remove("inode");
        assert!(serde_json::from_value::<FileObservationCheckpoint>(missing).is_err());
        let mut old_version = checkpoint.clone();
        old_version["schemaVersion"] = serde_json::json!(1);
        assert!(serde_json::from_value::<FileObservationCheckpoint>(old_version).is_err());
        let mut old_parent_shape = checkpoint.clone();
        old_parent_shape
            .as_object_mut()
            .unwrap()
            .remove("parentDirectoryIdentity");
        old_parent_shape["parentIdentity"] = serde_json::json!({
            "byteCount": 0,
            "modifiedNs": null,
            "device": null,
            "inode": null
        });
        assert!(serde_json::from_value::<FileObservationCheckpoint>(old_parent_shape).is_err());
        let mut extra = checkpoint;
        extra["internalCause"] = serde_json::json!("must not be accepted");
        assert!(serde_json::from_value::<FileObservationCheckpoint>(extra).is_err());
    }
}
