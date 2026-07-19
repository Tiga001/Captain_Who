//! Two-phase orchestration for acquiring and committing managed Skills.
//!
//! Inspection captures one exact, validated [`PreparedSkillAcquisition`] and
//! returns a safe preview. Commit consumes that snapshot; it never re-reads
//! the acquisition source. A bounded, expiring registry makes retries
//! idempotent without turning preparation IDs into permanent server state.
//!
//! Acquisition is an adapter boundary. Local directories are built in, while
//! future Git, archive, or registry adapters can register another provider and
//! still enter the same preview, acknowledgement, and installation transaction.

use super::acquisition_provenance::{
    SkillInstallationAuthority, SkillInstallationProvenance, SkillInstallationProvenanceView,
    SkillInstallationRefresh, SkillInstallationRefreshView,
};
use super::installation_service::{
    InstalledSkillRecord, SkillInstallationMutation, SkillInstallationOperation,
    SkillInstallationService, SkillInstallationServiceError,
};
#[cfg(test)]
use super::installation_session::SessionClock;
use super::installation_session::{
    duration_millis, InstallationSessionState, PreparationSlot, ResolutionSlot,
    SkillInstallationSessionConfig, SkillInstallationSessionStore,
};
use super::installed::USER_INSTALLED_SKILL_SOURCE_ID;
use super::model::{
    SkillId, SkillInstallationId, SkillInstallationRevision, SkillResourceKind, SkillRevision,
    SkillSourceId,
};
use super::package::MAX_SKILL_PACKAGE_BYTES;
use super::prepared::{PreparedSkillPackage, SkillPackagePreparationError};
use super::prepared_acquisition::PreparedSkillAcquisition;
use super::source_resolution::{SkillSourceCandidateId, SkillSourceResolutionId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

mod acquisition;
mod commit;
mod config;
mod error;
mod preparation;
mod preview;
mod workflow;

pub use acquisition::*;
pub use config::*;
pub use error::*;
pub use preview::*;
pub use workflow::*;

#[cfg(test)]
use commit::CommittingSlotRecovery;
#[cfg(test)]
use preparation::PreparingSlotRecovery;

#[cfg(test)]
mod tests;
