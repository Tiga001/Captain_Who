use crate::file_input::{
    evidence_from_bindings, materialize_agent_file_inputs, AgentFileInputExecutionContext,
    PreparedAgentFileInputs, AGENT_FILE_INPUT_ROOT_ENV,
};
use crate::system_paths::expand_system_path;
use crate::{
    AgentCancellationToken, AgentCommandArtifactChange, AgentCommandArtifactChangeKind,
    AgentCommandArtifactKind, AgentCommandArtifactMetadata, AgentCommandArtifactObservation,
    AgentCommandArtifactObservationCoverage, AgentCommandArtifactObservationKind,
    AgentCommandArtifactObservationPhase, AgentCommandArtifactObservationRequest,
    AgentCommandArtifactObservationStatus, AgentCommandArtifactObservationWarning,
    AgentCommandArtifactScope, AgentCommandArtifactSnapshotCoverage,
    AgentCommandArtifactValidation, AgentCommandArtifactValidationStatus,
    AgentCommandExpectedArtifactOutcome, AgentCommandExpectedArtifactOutcomeKind,
    AgentCommandOutputStream, AgentCommandRequest, AgentCommandRiskLevel,
    AgentCommandRuntimeBinding, AgentCommandRuntimeKind, AgentCommandRuntimeRequest,
    AgentCommandRuntimeResolution, AgentCommandSafetyPolicy, AgentPermissions, AgentReadPermission,
    AgentToolResult, AgentWritePermission, AGENT_COMMAND_ARTIFACT_OBSERVATION_SCHEMA_VERSION,
    AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod allowlist;
mod artifact_observer;
mod execution;
mod lexer;
mod managed_output_publication;
mod managed_runtime;
mod output_capture;
mod policy;
mod process_control;
mod risk;
mod runtime_profile;
mod segment;
mod session;
mod session_manager;
mod spawn_plan;
mod transcript;
mod types;

use allowlist::*;
use artifact_observer::CommandArtifactObserver;
pub(crate) use artifact_observer::{
    MAX_ADDITIONAL_ROOTS, MAX_EXPECTED_OUTPUTS, MAX_OBSERVATION_PATH_CHARS,
};
pub use execution::*;
use lexer::*;
pub use managed_output_publication::{
    publish_managed_command_outputs, snapshot_managed_command_output_baseline,
    MAX_MANAGED_COMMAND_OUTPUT_DEPTH, MAX_MANAGED_COMMAND_OUTPUT_ENTRIES,
    MAX_MANAGED_COMMAND_OUTPUT_FILES, MAX_MANAGED_COMMAND_OUTPUT_TOTAL_BYTES,
};
pub(crate) use managed_runtime::{
    infer_managed_artifact_builder_command, infer_managed_artifact_command_kind,
    infer_managed_pdf_command_kind, infer_managed_pdf_workspace_input,
    validate_command_runtime_request, validate_managed_artifact_builder_output_scope,
    validate_managed_artifact_command_shape,
};
pub use managed_runtime::{
    run_authorized_command_with_artifact_runtime,
    run_authorized_command_with_artifact_runtime_and_inputs,
    run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer,
};
pub use output_capture::{
    join_process_output_capture, materialize_process_tool_result_archive,
    process_output_spool_substitutions, spawn_process_output_capture,
    spawn_process_output_capture_with_observer, CapturedProcessOutput, ProcessOutputCaptureBudget,
    ProcessOutputCaptureHandle, ProcessOutputCaptureMetadata, ProcessOutputCapturePolicy,
    ProcessOutputObserver, ProcessOutputSpool, ProcessOutputSpoolSubstitution,
};
pub use policy::*;
pub(crate) use process_control::{
    configure_command_process_group, force_terminate_command_process_group,
    interrupt_command_process_group, terminate_command_process_group,
    try_wait_command_process_group, ManagedCommandChild,
};
use risk::*;
#[cfg(test)]
pub(crate) use runtime_profile::runtime_profile_revision;
pub(crate) use runtime_profile::{
    prepare_command_runtime_profile, validate_command_runtime_binding,
};
pub use runtime_profile::{
    CommandRuntimeProfileError, CommandRuntimeProfileResolver,
    COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH, COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE,
};
use segment::*;
pub use session::{
    CommandSessionError, CommandSessionId, CommandSessionLifecycleEvent,
    CommandSessionLifecycleObserver, CommandSessionPoll, CommandSessionProjection,
    CommandSessionScopeId, CommandSessionSnapshot, CommandSessionState, CommandStartOutcome,
    CommandTerminalResult,
};
pub use session_manager::{
    CommandSessionManager, CommandSessionManagerConfig, CommandSessionStartError,
    CommandStartOptions, CommandTerminationReport, DEFAULT_COMMAND_POLL_BYTES,
    DEFAULT_COMMAND_TRANSCRIPT_BYTES, DEFAULT_INITIAL_YIELD_MS, MAX_INITIAL_YIELD_MS,
    MIN_COMMAND_POLL_BYTES, MIN_INITIAL_YIELD_MS,
};
pub(crate) use spawn_plan::{CommandDirectLaunchPlan, CommandSpawnPlan};
pub use transcript::{CommandOutputBatch, CommandOutputChunk};
pub use types::*;

/// Projects an immutable command execution receipt into its canonical model-visible ToolResult.
///
/// Keeping this projection in the command domain ensures that live execution, durable audit
/// recovery, and startup settlement verification cannot disagree about whether an execution
/// succeeded or which error should be shown. User approval only authorizes an attempt; success is
/// derived exclusively from the observed process outcome.
pub fn command_tool_result(
    call_id: &str,
    command_result: &AgentCommandExecutionResult,
) -> AgentToolResult {
    let ok = command_result.exit_code == Some(0)
        && !command_result.timed_out
        && !command_result.cancelled
        && command_result.error.is_none();
    let mut tool_result = AgentToolResult {
        exact_archive_file: None,
        call_id: call_id.to_string(),
        tool: "run_command".to_string(),
        ok,
        result: Some(command_terminal_result_value(command_result)),
        error: if ok {
            None
        } else {
            command_result.error.clone().or_else(|| {
                Some(if command_result.cancelled {
                    "命令已取消。".to_string()
                } else if command_result.timed_out {
                    "命令执行超时。".to_string()
                } else {
                    "命令执行失败。".to_string()
                })
            })
        },
    };
    let substitutions = command_result.output_spool_substitutions();
    if !substitutions.is_empty() {
        match materialize_process_tool_result_archive(&tool_result, &substitutions) {
            Ok(file) => tool_result.exact_archive_file = file,
            Err(error) => {
                eprintln!("failed to materialize exact run_command output archive: {error}");
            }
        }
    }
    tool_result
}

fn command_terminal_result_value(
    command_result: &AgentCommandExecutionResult,
) -> serde_json::Value {
    let mut value = serde_json::to_value(command_result)
        .expect("AgentCommandExecutionResult must remain serializable");
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "status".to_string(),
            serde_json::Value::String("exited".to_string()),
        );
        // `history_open` is durable evidence, not an independently trusted projection field.
        // Reinsert it only after validating the route shape below. If durable evidence claims a
        // route but validation fails, retain only an explicit invalid marker: the storage resolver
        // must fail closed instead of mistaking the result for a non-Session command and creating
        // a second Exact Archive.
        let history_route_claimed = command_result.authoritative_archive_ref.is_some()
            || command_result.history_open.is_some();
        object.remove("historyOpen");
        // A live Host result still owns the raw archive identity, so derive the route from that
        // authority first. A deserialized action-audit receipt deliberately has only the opaque
        // route; accept it only when it decodes to the exact archive-start shape emitted below.
        // This keeps live and durable canonical ToolResults identical without letting a malformed
        // persisted string smuggle another conversation_history operation into the model result.
        let history_open = command_result
            .authoritative_archive_ref
            .as_deref()
            .and_then(|archive_ref| {
                match crate::storage::conversation_history_open::encode_archive_history_open(
                    archive_ref,
                    0,
                ) {
                    Ok(open) => Some(open),
                    Err(error) => {
                        eprintln!("failed to encode authoritative command history route: {error}");
                        None
                    }
                }
            })
            .or_else(|| validated_persisted_command_history_open(command_result));
        if let Some(open) = history_open {
            object.insert(
                "historyOpen".to_string(),
                serde_json::Value::String(open.clone()),
            );
            object.insert(
                "continueWith".to_string(),
                serde_json::json!({
                    "tool": "conversation_history",
                    "args": { "open": open }
                }),
            );
        } else if history_route_claimed {
            object.insert(
                "historyOpenInvalid".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }
    value
}

fn validated_persisted_command_history_open(
    command_result: &AgentCommandExecutionResult,
) -> Option<String> {
    let open = command_result.history_open.as_deref()?;
    match crate::storage::conversation_history_open::decode_history_open(open) {
        Ok(crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            start_char: 0,
            ..
        }) => Some(open.to_string()),
        Ok(_) => {
            eprintln!("ignored non-archive command history route in durable execution evidence");
            None
        }
        Err(error) => {
            eprintln!(
                "ignored invalid command history route in durable execution evidence: {error}"
            );
            None
        }
    }
}

/// Binds one Host-verified Exact Archive to a command execution without exposing its raw id.
///
/// The backend-only ref remains available until runtime archival finishes, while the deterministic
/// opaque route is the only identity serialized into action audit evidence or model projections.
pub fn bind_authoritative_command_archive(
    command_result: &mut AgentCommandExecutionResult,
    archive_ref: String,
) -> Result<(), String> {
    let history_open =
        crate::storage::conversation_history_open::encode_archive_history_open(&archive_ref, 0)?;
    command_result.authoritative_archive_ref = Some(archive_ref);
    command_result.history_open = Some(history_open);
    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests;
