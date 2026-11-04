//! Command-session tests grouped by ownership, observation, and durable settlement.

use super::*;
#[cfg(target_os = "macos")]
use mycopilot_core::artifact_runtime::{ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider};
#[cfg(target_os = "macos")]
use mycopilot_core::command::CommandRuntimeProfileResolver;
use mycopilot_core::command::{CommandAuthorizationSource, CommandSessionManagerConfig};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionModelReadRequest,
    AgentCommandSessionOutputAppend, AgentCommandSessionTerminalUpdate,
    AGENT_COMMAND_SESSION_SCHEMA_VERSION,
};
use mycopilot_core::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit;
use mycopilot_core::{
    AgentCommandArtifactKind, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationRequest, AgentCommandArtifactValidationStatus,
    AgentCommandExpectedArtifactOutcomeKind, AgentCommandOutputStream, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentCommandSessionAction, AgentCommandSessionExecutionControl,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutor, AgentCommandSessionGetInput, AgentCommandSessionOutputChunk,
    AgentCommandSessionSnapshot, AgentDisplayStatus, AgentModelBatchReceiptRecord,
    AgentWaitModelProjection, AgentWaitReadySnapshot, AgentWaitTargetSnapshot,
    ConversationCommandSessionLifecyclePhase, ConversationHistoryArchiveTraceMetadata,
    AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
};
use rusqlite::Connection;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Barrier;
use std::thread;
use std::time::{Duration, Instant};
use std::{collections::VecDeque, sync::Mutex as StdMutex};

mod support;
use support::*;

mod admission_reconciliation;
mod handoff_ownership;
mod interrupts;
mod lifecycle;
#[cfg(target_os = "macos")]
mod managed_pdf;
mod model_observation;
mod settlement_recovery;
mod settlement_scheduler;
mod wait_isolation;
