//! Safe, create-only materialization of revision-bound Skill resources.
//!
//! This module is deliberately downstream of [`SkillResourceSession`]. It
//! never accepts a managed-store path and never resolves mutable installation
//! state. It supports one exact logical URI as an atomic file publication and
//! a complete `templates/**` subtree as an atomic directory publication.

use super::digest::package_file_digest;
use super::model::{SkillResourceDescriptor, SkillResourceKind};
use super::package::{
    SkillPackagePath, MAX_SKILL_PACKAGE_BYTES, MAX_SKILL_PACKAGE_DIRECTORIES,
    MAX_SKILL_PACKAGE_FILES, MAX_SKILL_RESOURCE_FILE_BYTES,
};
use super::resource_runtime::{
    SkillPackageUri, SkillResourceError, SkillResourceListOptions, SkillResourcePath,
    SkillResourceSession, SkillResourceUri, MAX_SKILL_RESOURCE_LIST_PAGE_SIZE,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

mod domain;
mod plan;
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
mod unix;

pub use domain::{
    SkillMaterializationDestination, SkillMaterializationError, SkillMaterializationErrorCode,
    SkillMaterializationOutcome, SkillMaterializationRecovery, SkillMaterializationRequest,
    SkillMaterializationStatus, SkillMaterializedTreeEntry,
    SkillTemplateTreeMaterializationOutcome, SkillTemplateTreeMaterializationRequest,
    MAX_SKILL_MATERIALIZATION_FILE_BYTES, MAX_SKILL_MATERIALIZATION_TREE_BYTES,
    MAX_SKILL_MATERIALIZATION_TREE_DIRECTORIES, MAX_SKILL_MATERIALIZATION_TREE_FILES,
    SKILL_MATERIALIZATION_TREE_DIGEST_PREFIX,
};
use domain::{TreeFileFingerprint, TreeFingerprint, STAGING_NAME_PREFIX};
use plan::{prepare_template_tree, PreparedTemplateTree};

/// Stateless executor for revision-bound Skill resource materialization.
#[derive(Debug, Default, Clone, Copy)]
pub struct SkillResourceMaterializer;

impl SkillResourceMaterializer {
    pub const fn new() -> Self {
        Self
    }

    pub fn materialize(
        &self,
        session: &SkillResourceSession,
        request: &SkillMaterializationRequest,
    ) -> Result<SkillMaterializationOutcome, SkillMaterializationError> {
        let snapshot = session.read_verified_bytes(request.source())?;
        if snapshot.bytes.len() > MAX_SKILL_MATERIALIZATION_FILE_BYTES {
            return Err(SkillMaterializationError::Resource {
                source: Box::new(SkillResourceError::IntegrityMismatch {
                    uri: Box::new(request.source().clone()),
                    reason: format!(
                        "resource exceeds the {MAX_SKILL_MATERIALIZATION_FILE_BYTES}-byte materialization limit"
                    ),
                }),
            });
        }
        let descriptor = snapshot.descriptor;
        if descriptor.kind() != SkillResourceKind::Asset
            && !descriptor.path().starts_with("templates/")
        {
            return Err(SkillMaterializationError::SourceKindDenied {
                uri: Box::new(request.source().clone()),
                kind: descriptor.kind(),
            });
        }

        let status = materialize_file(
            request.workspace_root(),
            request.workspace_identity(),
            request.destination(),
            &descriptor,
            &snapshot.bytes,
        )?;
        Ok(SkillMaterializationOutcome {
            status,
            source: request.source().clone(),
            destination: request.destination().clone(),
            descriptor,
            bytes_written: if status == SkillMaterializationStatus::Created {
                u64::try_from(snapshot.bytes.len()).unwrap_or(u64::MAX)
            } else {
                0
            },
        })
    }

    /// Publishes a complete `templates/**` subtree as one create-only
    /// directory transaction. The destination directory is never merged or
    /// overwritten. An already-present byte-for-byte identical tree is an
    /// idempotent success.
    pub fn materialize_template_tree(
        &self,
        session: &SkillResourceSession,
        request: &SkillTemplateTreeMaterializationRequest,
    ) -> Result<SkillTemplateTreeMaterializationOutcome, SkillMaterializationError> {
        // Complete source verification deliberately precedes all workspace
        // filesystem activity. A tampered or unavailable package can never
        // leave a partial destination or staging tree.
        let prepared = prepare_template_tree(session, request)?;
        let status = materialize_tree(
            request.workspace_root(),
            request.workspace_identity(),
            request.destination(),
            &prepared,
        )?;
        let bytes_written = if status == SkillMaterializationStatus::Created {
            prepared.byte_length
        } else {
            0
        };
        Ok(SkillTemplateTreeMaterializationOutcome {
            status,
            source: request.source().clone(),
            source_prefix: request.source_prefix().clone(),
            destination: request.destination().clone(),
            entries: prepared
                .files
                .into_iter()
                .map(|file| SkillMaterializedTreeEntry {
                    source: file.source,
                    relative_path: file.relative_path,
                    descriptor: file.descriptor,
                })
                .collect(),
            byte_length: prepared.byte_length,
            bytes_written,
            plan_digest: prepared.plan_digest,
        })
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn materialize_file(
    workspace_root: &Path,
    workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    destination: &SkillMaterializationDestination,
    descriptor: &SkillResourceDescriptor,
    bytes: &[u8],
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    unix::materialize_file(
        workspace_root,
        workspace_identity,
        destination,
        descriptor,
        bytes,
    )
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn materialize_file(
    _workspace_root: &Path,
    _workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    _destination: &SkillMaterializationDestination,
    _descriptor: &SkillResourceDescriptor,
    _bytes: &[u8],
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    Err(SkillMaterializationError::UnsupportedPlatform)
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn materialize_tree(
    workspace_root: &Path,
    workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    destination: &SkillMaterializationDestination,
    prepared: &PreparedTemplateTree,
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    unix::materialize_tree(workspace_root, workspace_identity, destination, prepared)
}

#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn materialize_tree(
    _workspace_root: &Path,
    _workspace_identity: Option<&crate::file_change::FileChangeDirectoryIdentity>,
    _destination: &SkillMaterializationDestination,
    _prepared: &PreparedTemplateTree,
) -> Result<SkillMaterializationStatus, SkillMaterializationError> {
    Err(SkillMaterializationError::UnsupportedPlatform)
}

#[cfg(all(
    test,
    any(target_vendor = "apple", target_os = "linux", target_os = "android")
))]
mod tests;
