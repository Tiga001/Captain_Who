mod api;
mod attachments;
mod checkpoint;
mod command_dispatch;
mod context_compaction;
mod context_compaction_model;
mod context_materials;
mod conversation_world_state;
mod events;
mod extensions;
mod file_transactions;
mod output_budget;
mod preparation;
mod tool_failure_guard;
mod tool_flow;
mod tool_input_stream;
mod trace;
mod world_state;

pub use api::*;
use command_dispatch::*;
use context_materials::*;
use conversation_world_state::*;
use events::*;
use output_budget::resolve_output_budget;
pub(crate) use output_budget::resolve_profile_output_budget;
use preparation::*;
use trace::*;
use world_state::*;

use crate::cancellation::AgentCancellationToken;
use crate::command::{CommandAuthorizationSource, CommandPolicyDecision};
use crate::context::{
    AgentContextBaseline, AgentContextWindowToolProjection, AgentConversationContextState,
    ContextAssembler, ContextAssemblyInput, ContextAttachments, ContextBudgetReport,
    ContextCapacityDetector, ContextCompactionPlan, ContextCompactionPlanStatus,
    ContextCompactionPlanner, ContextCompactionQuery, ContextFrame, ContextItem, ContextMetadata,
    ContextOrigin, ContextRetention, ContextScope, ContextSource, ModelToolResultGate,
    ModelToolResultRecovery, MODEL_TOOL_RESULT_MAX_TOKENS,
};
use crate::conversation_trace::{
    conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection,
    trace_attachments_from_input, ConversationHistoryArchiveTraceMetadata,
    ConversationTraceRecorder,
};
use crate::file_change::FileObservationCheckpoint;
use crate::file_change_support::{
    file_change_approval_route, file_change_snapshot, FileChangeApprovalRoute,
};
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, is_repairable_empty_model_action,
    LlmChatRequest, LlmMessage, LlmMessageRole, LlmStreamEvent,
};
use crate::model_request_observation::{
    ModelRequestEstimate, ModelRequestObservation, ModelRequestObservationBuilder,
    ModelRequestPurpose, ModelRequestToolSetObservation,
};
use crate::prompts::build_system_prompt_with_collaboration;
use crate::protocol::{
    AgentApprovalStatus, AgentAutomationExecutionContext, AgentBuiltinExecutionPermission,
    AgentChatInput, AgentChatMessage, AgentChatOutput, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentContextCompactionEventOutcome, AgentContextWindowSnapshot,
    AgentError, AgentEvent, AgentExtensionSnapshot, AgentFileChangeOperation, AgentPermissions,
    AgentPromptPreferences, AgentProposedAction, AgentReadPermission, AgentResult, AgentRunContext,
    AgentRunStatus, AgentSkillActivation, AgentSkillScriptPreflightStatus, AgentSkillScriptRequest,
    AgentSkillScriptSourceKind, AgentSkillScriptTrust, AgentSteerInput, AgentToolApprovalMode,
    AgentToolCall, AgentToolDefinition, AgentToolIdentity, AgentToolResult, AgentWritePermission,
};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey,
};
use crate::revision::content_revision;
use crate::storage::conversation_history_archive_repository::{
    ConversationHistoryArchiveFileInput, ConversationHistoryArchiveInput,
};
use crate::storage::now_ms;
use crate::storage::service::StorageService;
use crate::tools::{EffectiveToolSet, ToolExecutionContext, ToolRegistry, ToolUnavailability};
use crate::{
    canonical_pending_action_id, resolve_provider_runtime_capabilities,
    AgentCollaborationRuntimeServices, ConversationTraceSnapshot, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    ProviderContinuationRequirement, ProviderPrivateReplaySemantics, ProviderUsageSemantics,
};
use attachments::{build_attachment_context, AttachmentContext};
use checkpoint::{
    checkpoint_continuation_projection, continuation_result_sequence,
    create_run_checkpoint_with_file_observations, restore_run_checkpoint_with_model_projection,
    CheckpointContinuationProjection, QueuedToolCall, RestoredRunCheckpoint, RunCheckpointState,
    ToolCallBatch, ToolCallBatchClaim,
};
use context_compaction::{ContextCompactionExecution, ContextCompactionExecutor};
use extensions::{
    ModelInputCapacity, ModelRequestContext, RuntimeEffect, RuntimeExtensionEvent,
    RuntimeExtensions,
};
use file_transactions::{
    FileTransactionRunGuard, FileTransactionState, MAX_RESPONSE_FENCE_CORRECTIONS,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use tool_failure_guard::ToolFailureGuard;
use tool_flow::{
    approve_proposed_action, cancellation_preempts_tool_result, cancelled_output, done_event,
    execute_host_action_on_blocking_thread, execute_registered_tool, extract_reason_from_args,
    failed_tool_call_result, generate_run_id, llm_image_message_from_tool_result,
    redact_tool_result_for_event, sanitize_temperature, state_event,
    tool_call_bindings_from_response,
};
use tool_input_stream::ToolInputStreamObservers;

const DEFAULT_TEMPERATURE: f32 = 0.6;
const MAX_TOOL_ITERATIONS: usize = 10_000;
const MAX_CONTEXT_COMPACTION_ATTEMPTS_PER_REQUEST: usize = 3;

fn pending_file_change_observation(
    action: &AgentProposedAction,
) -> Option<&FileObservationCheckpoint> {
    let AgentProposedAction::FileChange { file_change } = action else {
        return None;
    };
    matches!(
        file_change.operation,
        AgentFileChangeOperation::Update | AgentFileChangeOperation::Delete
    )
    .then_some(&file_change.execution.observation)
}

fn merge_provider_usage(
    total: &mut Option<crate::protocol::AgentUsage>,
    next: Option<crate::protocol::AgentUsage>,
    usage_semantics: ProviderUsageSemantics,
) {
    usage_semantics.merge_usage(total, next);
}

#[derive(Clone, Copy)]
struct ProviderContinuationResumeRequirement<'a> {
    required_refs: Option<&'a [crate::protocol::ProviderContinuationRef]>,
    current_assistant_turn_id: Option<&'a str>,
}

fn hydrate_provider_continuation_history(
    context: &mut ContextFrame,
    profile: &ProviderProfileConfig,
    protocol: &ProviderProtocolKey,
    conversation_id: Option<&str>,
    storage: Option<&crate::storage::service::StorageService>,
    vault: Option<&crate::ProviderContinuationVault>,
    resume: ProviderContinuationResumeRequirement<'_>,
) -> AgentResult<Option<crate::llm::LlmAssistantTurn>> {
    let ProviderContinuationResumeRequirement {
        required_refs,
        current_assistant_turn_id,
    } = resume;
    let capabilities = resolve_provider_runtime_capabilities(protocol)
        .map_err(|error| AgentError::new(format!("Provider runtime capabilities 无效：{error}")))?;
    if capabilities.private_replay() == ProviderPrivateReplaySemantics::None {
        let has_provider_state = match (conversation_id, vault, storage) {
            (Some(conversation_id), Some(vault), _) => vault
                .has_replayable_for_conversation(conversation_id)
                .map_err(provider_continuation_runtime_error)?,
            (Some(conversation_id), None, Some(storage)) => storage
                .has_replayable_provider_continuations_for_conversation(conversation_id)
                .map_err(|_| {
                    provider_continuation_runtime_error(
                        crate::ProviderContinuationStoreError::RepositoryUnavailable,
                    )
                })?,
            _ => false,
        };
        if required_refs.is_none_or(|refs| refs.is_empty()) && !has_provider_state {
            return Ok(None);
        }
        return Err(AgentError::structured(
            "provider_context_boundary_required",
            "冻结的 Provider continuation 不能通过当前 Profile 回放。",
            json!({ "type": "providerContextBoundary" }),
        ));
    }
    if let (Some(conversation_id), Some(storage)) = (conversation_id, storage) {
        let visible_projections = context.journal_provider_continuation_projections();
        if storage
            .has_released_provider_continuations_for_projections(
                conversation_id,
                &visible_projections,
            )
            .map_err(|_| {
                provider_continuation_runtime_error(
                    crate::ProviderContinuationStoreError::RepositoryUnavailable,
                )
            })?
        {
            return Err(AgentError::structured(
                "provider_context_boundary_required",
                "当前上下文会暴露已释放的 Provider Assistant Turn。",
                json!({
                    "type": "providerContextBoundary",
                    "recovery": "restartFromSafeContextBoundary"
                }),
            ));
        }
    }
    let has_explicit_replay_state = required_refs.is_some_and(|refs| !refs.is_empty())
        || !context.provider_continuation_refs()?.is_empty()
        || context.has_tool_bearing_assistant_turns();
    let has_stored_replay_state = match (conversation_id, vault, storage) {
        (Some(conversation_id), Some(vault), _) => vault
            .has_replayable_for_conversation(conversation_id)
            .map_err(provider_continuation_runtime_error)?,
        (Some(conversation_id), None, Some(storage)) => storage
            .has_replayable_provider_continuations_for_conversation(conversation_id)
            .map_err(|_| {
                provider_continuation_runtime_error(
                    crate::ProviderContinuationStoreError::RepositoryUnavailable,
                )
            })?,
        _ => false,
    };
    if !has_explicit_replay_state && !has_stored_replay_state {
        return Ok(None);
    }
    let conversation_id = conversation_id.ok_or_else(|| {
        provider_continuation_runtime_error(crate::ProviderContinuationStoreError::InvalidBinding)
    })?;
    let vault = vault.ok_or_else(|| {
        provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::CredentialUnavailable,
        )
    })?;
    let loaded = vault
        .list_replayable_for_conversation(conversation_id, protocol)
        .map_err(provider_continuation_runtime_error)?;
    // Freeze durable visibility before restoring anything. In particular, inserting an empty
    // steer-boundary Assistant must not make a second, corrupt narration projection at the same
    // sequence appear eligible later in this loop.
    let visible = loaded
        .iter()
        .map(|loaded_turn| match loaded_turn.projection {
            Some(projection) => context.contains_provider_continuation_projection(
                &loaded_turn.assistant_message_id,
                projection,
            ),
            None => context.contains_trace_for_assistant_message(&loaded_turn.assistant_message_id),
        })
        .collect::<Vec<_>>();
    let mut restored_refs = std::collections::BTreeSet::new();
    let mut current_assistant_turn = None;
    for (loaded_turn, projection_is_visible) in loaded.into_iter().zip(visible) {
        if !projection_is_visible {
            continue;
        }
        restored_refs.insert(loaded_turn.continuation_ref.id.clone());
        if current_assistant_turn_id == Some(loaded_turn.assistant_turn.stable_id().as_str()) {
            current_assistant_turn = Some(loaded_turn.assistant_turn.clone());
        }
        context.restore_provider_assistant_turn(
            &loaded_turn.assistant_message_id,
            loaded_turn.projection,
            loaded_turn.assistant_turn,
        )?;
    }
    if let Some(required_refs) = required_refs {
        let required_ref_ids = required_refs
            .iter()
            .map(|continuation_ref| continuation_ref.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let restored_ref_ids = restored_refs
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if required_ref_ids != restored_ref_ids {
            return Err(provider_continuation_runtime_error(
                crate::ProviderContinuationStoreError::PayloadNotFound,
            ));
        }
    }
    context.ensure_tool_bearing_turns_replayable(protocol, profile.reasoning_mode())?;
    if current_assistant_turn_id.is_some() && current_assistant_turn.is_none() {
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::CheckpointStateMissing,
        ));
    }
    Ok(current_assistant_turn)
}

#[allow(clippy::too_many_arguments)]
fn stage_ordinary_provider_assistant_turn(
    mut turn: crate::llm::LlmAssistantTurn,
    capabilities: crate::ProviderRuntimeCapabilities,
    reasoning_mode: crate::ReasoningMode,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
    run_id: &str,
    request_index: usize,
    protocol: &ProviderProtocolKey,
    vault: Option<&Arc<crate::ProviderContinuationVault>>,
    projection: crate::ProviderContinuationProjection,
) -> AgentResult<(
    crate::llm::LlmAssistantTurn,
    Option<PendingProviderContinuationHandoff>,
)> {
    if !turn.provider_tool_calls().is_empty() {
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::InvalidTurn,
        ));
    }
    let has_provider_continuation = turn.provider_continuation().is_some();
    let turn_policy = capabilities.classify_turn(
        !turn.provider_tool_calls().is_empty(),
        has_provider_continuation,
        reasoning_mode,
    );
    if matches!(
        (
            turn_policy.continuation_requirement(),
            has_provider_continuation
        ),
        (ProviderContinuationRequirement::Required, false)
            | (ProviderContinuationRequirement::Forbidden, true)
    ) {
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::InvalidTurn,
        ));
    }
    if !turn_policy.requires_private_replay() {
        return Ok((turn, None));
    }

    let conversation_id = conversation_id.ok_or_else(|| {
        provider_continuation_runtime_error(crate::ProviderContinuationStoreError::InvalidBinding)
    })?;
    let assistant_message_id = assistant_message_id.ok_or_else(|| {
        provider_continuation_runtime_error(crate::ProviderContinuationStoreError::InvalidBinding)
    })?;
    let vault = vault.cloned().ok_or_else(|| {
        provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::CredentialUnavailable,
        )
    })?;
    let request_index = u64::try_from(request_index).map_err(|_| {
        provider_continuation_runtime_error(crate::ProviderContinuationStoreError::InvalidBinding)
    })?;
    let assistant_turn_id = turn.stable_id();
    let assistant_turn_digest = turn.stable_digest();
    let binding = crate::provider_continuation_store::ProviderContinuationBinding {
        conversation_id,
        assistant_message_id,
        run_id,
        request_index,
        assistant_turn_id: &assistant_turn_id,
        assistant_turn_digest: &assistant_turn_digest,
        provider_protocol: protocol,
    };
    let continuation_ref = vault
        .persist_staged_with_projection(binding, projection, &turn)
        .map_err(provider_continuation_runtime_error)?
        .ok_or_else(|| {
            provider_continuation_runtime_error(
                crate::ProviderContinuationStoreError::PayloadNotFound,
            )
        })?;
    turn = match turn.with_provider_continuation_ref(continuation_ref.clone()) {
        Ok(turn) => turn,
        Err(error) => {
            let _ = vault.release(
                &continuation_ref,
                crate::provider_continuation_store::ProviderContinuationOwner {
                    conversation_id,
                    assistant_message_id,
                    run_id,
                },
                now_ms(),
            );
            return Err(error);
        }
    };
    Ok((
        turn,
        Some(PendingProviderContinuationHandoff {
            vault,
            continuation_ref,
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            request_index,
            assistant_turn_id,
            assistant_turn_digest,
            provider_protocol: protocol.clone(),
        }),
    ))
}

fn provider_continuation_runtime_error(error: crate::ProviderContinuationStoreError) -> AgentError {
    use crate::ProviderContinuationStoreError as StoreError;
    let (code, message, recovery) = match error {
        StoreError::PayloadNotFound
        | StoreError::PayloadReleased
        | StoreError::CheckpointStateMissing => (
            "provider_continuation_missing",
            "Provider continuation 不存在，已停止工具执行和模型请求。",
            "restartFromSafeContextBoundary",
        ),
        StoreError::InvalidBinding | StoreError::PayloadConflict => (
            "provider_context_boundary_required",
            "Provider continuation 与当前冻结协议不兼容。",
            "compactIncompatibleToolHistory",
        ),
        StoreError::InvalidReference
        | StoreError::InvalidTurn
        | StoreError::PayloadTooLarge
        | StoreError::InvalidEnvelope
        | StoreError::AuthenticationFailed
        | StoreError::CompressionFailed => (
            "provider_continuation_corrupt",
            "Provider continuation 无法通过完整性校验，已停止工具执行和模型请求。",
            "restartFromSafeContextBoundary",
        ),
        StoreError::CredentialUnavailable
        | StoreError::RepositoryUnavailable
        | StoreError::RandomnessUnavailable => (
            "provider_continuation_unavailable",
            "Provider continuation 私有存储当前不可用，已停止工具执行和模型请求。",
            "retryWhenPrivateStoreIsAvailable",
        ),
    };
    AgentError::structured(
        code,
        message,
        json!({
            "type": "providerContinuation",
            "recovery": recovery
        }),
    )
}

struct LlmRequestTemplate {
    api_url: String,
    api_token: String,
    model: String,
    api_style: crate::protocol::AgentApiStyle,
    context_window_tokens: Option<u32>,
    max_tokens: Option<u32>,
    reserved_output_tokens: u32,
    temperature: f32,
    stream: bool,
    stable_tools: Vec<AgentToolDefinition>,
    provider_profile_config: ProviderProfileConfig,
    provider_protocol_key: ProviderProtocolKey,
}

impl LlmRequestTemplate {
    fn request(
        &self,
        context: ContextFrame,
        dynamic_tools: &[AgentToolDefinition],
    ) -> LlmChatRequest {
        let mut tools =
            Vec::with_capacity(self.stable_tools.len().saturating_add(dynamic_tools.len()));
        tools.extend(self.stable_tools.iter().cloned());
        tools.extend(dynamic_tools.iter().cloned());
        LlmChatRequest {
            api_url: self.api_url.clone(),
            api_token: self.api_token.clone(),
            provider_profile: self.provider_profile_config.clone(),
            provider_protocol: self.provider_protocol_key.clone(),
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            stream: self.stream,
            messages: context.into_model_request_messages(),
            tools,
        }
    }
}

struct PreparedLlmRequest {
    template: LlmRequestTemplate,
    context: ContextFrame,
    next_model_request_index: usize,
    tool_batch: ToolCallBatch,
    conversation_trace: ConversationTraceRecorder,
    provider_continuation_refs: Option<Vec<crate::protocol::ProviderContinuationRef>>,
}

struct PreparedRuntimeCapabilities {
    runtime_extensions: RuntimeExtensions,
    tool_registry: Arc<ToolRegistry>,
    /// Registry definitions after dynamic-capability permission filtering. Stable definitions
    /// are never removed or rewritten by composer permissions; authorization remains a runtime
    /// and host concern. This superset is partitioned at every model-request boundary.
    tool_definitions: Vec<AgentToolDefinition>,
    initial_tool_set: EffectiveToolSet,
    command_auto_approve: bool,
    command_permissions: AgentPermissions,
    command_workspace_root: Option<PathBuf>,
    patch_auto_approve: bool,
}

struct AgentSteerInputCloseGuard {
    input: Option<AgentSteerInputQueue>,
}

impl AgentSteerInputCloseGuard {
    fn new(input: Option<AgentSteerInputQueue>) -> Self {
        Self { input }
    }
}

impl Drop for AgentSteerInputCloseGuard {
    fn drop(&mut self) {
        if let Some(input) = &self.input {
            input.close();
        }
    }
}

pub struct AgentRuntime {
    max_tool_iterations: usize,
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self {
            max_tool_iterations: MAX_TOOL_ITERATIONS,
        }
    }
}

// The runtime driver remains one lexical state machine: moving it to a source shard does not add
// a second owner for request ordering, continuation state, cancellation, or effect settlement.
include!("runtime/driver.rs");
include!("runtime/tool_settlement.rs");
include!("runtime/projection_and_steering.rs");

#[cfg(test)]
mod tests;
