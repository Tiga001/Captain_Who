mod api;
mod attachments;
mod checkpoint;
mod command_dispatch;
mod context_compaction;
mod context_compaction_model;
mod events;
mod extensions;
mod file_transactions;
mod preparation;
mod tool_failure_guard;
mod tool_flow;
mod tool_input_stream;
mod trace;
mod world_state;

pub use api::*;
use command_dispatch::*;
use events::*;
use preparation::*;
use trace::*;
use world_state::*;

use crate::cancellation::AgentCancellationToken;
use crate::command::{
    evaluate_command_policy_with_context, CommandAuthorizationSource, CommandPolicyDecision,
};
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
use crate::file_write::{file_write_approval_route, FileWriteApprovalRoute};
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, is_repairable_empty_model_action,
    LlmChatRequest, LlmMessage, LlmMessageRole, LlmStreamEvent,
};
use crate::model_request_observation::{
    ModelRequestEstimate, ModelRequestObservation, ModelRequestObservationBuilder,
    ModelRequestPurpose, ModelRequestToolSetObservation,
};
use crate::prompts::{build_system_prompt_with_collaboration, collaboration_harness_section};
use crate::protocol::{
    AgentApprovalStatus, AgentAutomationExecutionContext, AgentBuiltinExecutionPermission,
    AgentChatInput, AgentChatMessage, AgentChatOutput, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentContextCompactionEventOutcome, AgentContextWindowSnapshot,
    AgentError, AgentEvent, AgentExtensionSnapshot, AgentPermissions, AgentPromptPreferences,
    AgentProposedAction, AgentReadPermission, AgentResult, AgentRunContext, AgentRunStatus,
    AgentSkillActivation, AgentSkillScriptPreflightStatus, AgentSkillScriptRequest,
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
    resolve_provider_runtime_capabilities, AgentCollaborationRuntimeServices,
    ConversationTraceSnapshot, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, ProviderContinuationRequirement,
    ProviderPrivateReplaySemantics, ProviderUsageSemantics,
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
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex;
use tool_failure_guard::ToolFailureGuard;
use tool_flow::{
    approve_proposed_action, cancellation_preempts_tool_result, cancelled_output, done_event,
    execute_host_action_on_blocking_thread, execute_registered_tool, extract_reason_from_args,
    failed_tool_call_result, file_draft_from_tool_result, generate_run_id,
    llm_image_message_from_tool_result, redact_tool_result_for_event, sanitize_max_tokens,
    sanitize_temperature, state_event, tool_call_bindings_from_response,
};
use tool_input_stream::ToolInputStreamObservers;

const DEFAULT_MAX_TOKENS: u32 = 30_000;
const MAX_MAX_TOKENS: u32 = 128_000;
const DEFAULT_TEMPERATURE: f32 = 0.6;
const MAX_TOOL_ITERATIONS: usize = 10_000;
const MAX_CONTEXT_COMPACTION_ATTEMPTS_PER_REQUEST: usize = 3;

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
    if !context.has_tool_bearing_assistant_turns() {
        if required_refs.is_none_or(|refs| refs.is_empty()) {
            return Ok(None);
        }
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::PayloadNotFound,
        ));
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
    let mut restored_refs = std::collections::BTreeSet::new();
    let mut current_assistant_turn = None;
    for loaded_turn in loaded {
        if !context.contains_trace_for_assistant_message(&loaded_turn.assistant_message_id) {
            continue;
        }
        restored_refs.insert(loaded_turn.continuation_ref.id.clone());
        if current_assistant_turn_id == Some(loaded_turn.assistant_turn.stable_id().as_str()) {
            current_assistant_turn = Some(loaded_turn.assistant_turn.clone());
        }
        context.restore_provider_assistant_turn(
            &loaded_turn.assistant_message_id,
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
    context.ensure_tool_bearing_turns_replayable(protocol, profile.reasoning.mode)?;
    if current_assistant_turn_id.is_some() && current_assistant_turn.is_none() {
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::CheckpointStateMissing,
        ));
    }
    Ok(current_assistant_turn)
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
    max_tokens: u32,
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
            messages: context.into_messages(),
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

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

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

impl AgentRuntime {
    pub async fn send_chat(&self, input: AgentChatInput) -> AgentResult<AgentChatOutput> {
        self.send_chat_with_events(input, None, None).await
    }

    pub async fn send_chat_with_events(
        &self,
        input: AgentChatInput,
        run_id: Option<String>,
        emitter: Option<AgentEventEmitter>,
    ) -> AgentResult<AgentChatOutput> {
        self.send_chat_with_events_and_cancellation(
            input,
            run_id,
            emitter,
            AgentCancellationToken::new(),
            None,
        )
        .await
    }

    pub async fn send_chat_with_events_and_cancellation(
        &self,
        mut input: AgentChatInput,
        run_id: Option<String>,
        emitter: Option<AgentEventEmitter>,
        cancellation_token: AgentCancellationToken,
        host_services: Option<AgentRuntimeHostServices>,
    ) -> AgentResult<AgentChatOutput> {
        let AgentRuntimeHostServices {
            host_executor,
            storage,
            trace_observer,
            model_request_observer,
            context_window_observer,
            context_compaction_services,
            provider_continuation_vault,
            skill_resources,
            skill_activation_resolver,
            office_engine,
            image_generation_execution,
            skill_installation_prepare,
            skill_installation_commit,
            mcp_tools,
            builtin_capabilities,
            command_runtime_profile_resolver,
            command_session_executor,
            steer_input,
            collaboration_inbox,
            mut agent_collaboration,
            automation_report_sink,
        } = host_services.unwrap_or_default();
        let _steer_input_close_guard = AgentSteerInputCloseGuard::new(steer_input.clone());
        let run_id = run_id.unwrap_or_else(generate_run_id);
        let restore_trace_conversation_id = input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.clone());
        let restore_trace_assistant_message_id = input.assistant_message_id.clone();
        let trace_run_id = run_id.clone();
        let checkpoint_api_style = input
            .api_style
            .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
        let checkpoint_model_tool_result_gate =
            ContextCapacityDetector::for_model(input.model.trim(), checkpoint_api_style, &[])
                .model_tool_result_gate();
        let checkpoint_prefix_trace = conversation_trace_checkpoint_prefix_from_input(&input);
        let continuation_archive_metadata =
            load_continuation_archive_metadata(&input, storage.as_deref()).map_err(|error| {
                attach_failed_runtime_trace(
                    error,
                    &checkpoint_prefix_trace,
                    &trace_run_id,
                    restore_trace_conversation_id.as_deref(),
                    restore_trace_assistant_message_id.as_deref(),
                )
            })?;
        let setup_conversation_trace = conversation_trace_from_input_checkpoint(
            &input,
            &checkpoint_model_tool_result_gate,
            &continuation_archive_metadata,
        )
        .map_err(|error| {
            attach_failed_runtime_trace(
                error,
                &checkpoint_prefix_trace,
                &trace_run_id,
                restore_trace_conversation_id.as_deref(),
                restore_trace_assistant_message_id.as_deref(),
            )
        })?;
        let shared_context_baseline =
            publish_trace_recorder_snapshot(&setup_conversation_trace, trace_observer.as_ref())?;
        let restored_checkpoint = restore_input_checkpoint(
            &mut input,
            &run_id,
            &checkpoint_model_tool_result_gate,
            &continuation_archive_metadata,
        )
        .map_err(|error| {
            attach_failed_runtime_trace(
                error,
                &setup_conversation_trace,
                &trace_run_id,
                restore_trace_conversation_id.as_deref(),
                restore_trace_assistant_message_id.as_deref(),
            )
        })?;
        if let Some(restored) = restored_checkpoint.as_ref() {
            // Approval resume continues the backend authority frozen in schema-v6 checkpoint.
            // Newer UI/settings payloads cannot silently change permissions, workspace,
            // attachment authority or model capabilities in the middle of one logical run.
            input.context = restored.run_context.clone();
            input.model_capabilities = restored.model_capabilities;
            input.provider_profile_config = Some(restored.provider_profile_config.clone());
            input.provider_protocol_key = Some(restored.provider_protocol_key.clone());
            agent_collaboration = match (
                agent_collaboration.take(),
                restored.collaboration_run_snapshot.as_ref(),
            ) {
                (Some(services), Some(snapshot)) => {
                    Some(services.with_run_snapshot(snapshot).map_err(|error| {
                        AgentError::new(format!(
                            "无法恢复运行检查点的 Agent collaboration 授权：{error}"
                        ))
                    })?)
                }
                (None, None) => None,
                _ => {
                    return Err(AgentError::new(
                        "无法恢复运行检查点：Agent collaboration Host capability 与冻结授权不一致。",
                    ));
                }
            };
        }
        let mut run_context = input.context.clone();
        let model_capabilities = input.model_capabilities;
        let trace_conversation_id = run_context
            .as_ref()
            .and_then(|context| context.conversation_id.clone());
        let trace_assistant_message_id = input.assistant_message_id.clone();
        let (observation_conversation_id, observation_assistant_message_id) =
            match (&trace_conversation_id, &trace_assistant_message_id) {
                (Some(conversation_id), Some(assistant_message_id)) => (
                    Some(conversation_id.clone()),
                    Some(assistant_message_id.clone()),
                ),
                _ => (None, None),
            };
        let extension_snapshots = restored_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.extension_snapshots.as_slice())
            .unwrap_or_default();
        let PreparedRuntimeCapabilities {
            mut runtime_extensions,
            tool_registry,
            tool_definitions: permitted_tool_definitions,
            initial_tool_set,
            command_auto_approve,
            command_permissions,
            command_workspace_root,
            patch_auto_approve,
        } = prepare_runtime_capabilities_with_skills(
            &input,
            &run_id,
            extension_snapshots,
            RuntimeCapabilityServices {
                host_actions_available: host_executor.is_some(),
                office_engine,
                image_generation_execution,
                skill_installation_prepare,
                skill_installation_commit,
                skill_activation_resolver,
                skill_resources: skill_resources.clone(),
                mcp_tools,
                builtin_capabilities,
                agent_collaboration_enabled: agent_collaboration.is_some(),
                automation_report_sink,
            },
        )
        .map_err(|error| {
            attach_failed_runtime_trace(
                error,
                &setup_conversation_trace,
                &trace_run_id,
                trace_conversation_id.as_deref(),
                trace_assistant_message_id.as_deref(),
            )
        })?;
        let restored_batch_tool_set = if let Some(restored) = restored_checkpoint.as_ref() {
            validate_resumed_tool_provenance(&setup_conversation_trace, &tool_registry).map_err(
                |error| {
                    attach_failed_runtime_trace(
                        error,
                        &setup_conversation_trace,
                        &trace_run_id,
                        trace_conversation_id.as_deref(),
                        trace_assistant_message_id.as_deref(),
                    )
                },
            )?;
            Some(
                initial_tool_set
                    .restore_frozen_checkpoint(&restored.tool_set)
                    .map_err(|error| {
                        attach_failed_runtime_trace(
                            error,
                            &setup_conversation_trace,
                            &trace_run_id,
                            trace_conversation_id.as_deref(),
                            trace_assistant_message_id.as_deref(),
                        )
                    })?,
            )
        } else {
            None
        };
        let stable_tool_revision = initial_tool_set.stable_revision().to_string();
        // A restored extension snapshot may already contain effects from siblings that completed
        // before Approval. Keep draining the paused response under its request-boundary ToolSet;
        // the loop recomputes from the newer extension state only at the next model boundary.
        let mut effective_tool_set = restored_batch_tool_set.unwrap_or(initial_tool_set);
        let resumed_world_state_epoch = restored_checkpoint.is_some();
        let mut run_world_state = match restored_checkpoint.as_ref() {
            Some(restored) => RunWorldStateTracker::from_checkpoint(
                format!("{run_id}:world-state:resume"),
                &restored.run_world_state,
            )?,
            None => RunWorldStateTracker::new_with_extension_sections(
                format!("{run_id}:world-state"),
                &input,
                &effective_tool_set,
                runtime_extensions.world_state_sections()?,
            )?,
        };
        let initial_run_world_state =
            (!resumed_world_state_epoch).then(|| run_world_state.snapshot().clone());
        let mut tool_definitions = effective_tool_set.all_definitions();
        let mut event_stream = AgentEventStream::new(emitter);
        event_stream.emit(AgentEvent::Started {
            run_id: run_id.clone(),
            tool_definitions: tool_registry.renderer_event_definitions(&tool_definitions),
        });
        event_stream.emit(AgentEvent::ToolSetChanged {
            run_id: run_id.clone(),
            stable_revision: effective_tool_set.stable_revision().to_string(),
            dynamic_revision: effective_tool_set.dynamic_revision().to_string(),
            effective_revision: effective_tool_set.revision().to_string(),
            tool_definitions: tool_registry.renderer_event_definitions(&tool_definitions),
        });
        let mut emitted_tool_set_revision = effective_tool_set.revision().to_string();
        let transaction_storage = storage.clone();
        let context_window_configured = input.context_window_tokens.is_some();
        let context_window_indicator_enabled = input.context_window_indicator_enabled;
        let mut file_transaction_guard = FileTransactionRunGuard::new(
            transaction_storage.clone(),
            run_id.clone(),
            trace_conversation_id.clone(),
            run_context
                .as_ref()
                .and_then(|context| context.project_id.clone()),
            cancellation_token.clone(),
        );
        let file_observations = restored_checkpoint
            .as_ref()
            .map(|checkpoint| Arc::clone(&checkpoint.file_observations))
            .unwrap_or_else(|| Arc::new(crate::file_change::FileObservationRegistry::default()));
        let PreparedLlmRequest {
            template: llm_request,
            context: mut active_context,
            mut next_model_request_index,
            mut tool_batch,
            conversation_trace,
            provider_continuation_refs: required_provider_continuation_refs,
        } = build_llm_request(
            input,
            effective_tool_set.stable_definitions(),
            restored_checkpoint,
            shared_context_baseline,
            initial_run_world_state,
        )
        .map_err(|error| {
            attach_failed_runtime_trace(
                error,
                &setup_conversation_trace,
                &trace_run_id,
                trace_conversation_id.as_deref(),
                trace_assistant_message_id.as_deref(),
            )
        })?;
        if !resumed_world_state_epoch {
            if let Some(services) = agent_collaboration.as_ref() {
                active_context.push(ContextItem::text(
                    LlmMessageRole::System,
                    collaboration_harness_section(&services.caller, &services.selector_directory),
                    ContextSource::RuntimeGuard,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ));
            }
        }
        let provider_runtime_capabilities = resolve_provider_runtime_capabilities(
            &llm_request.provider_protocol_key,
        )
        .map_err(|error| AgentError::new(format!("Provider runtime capabilities 无效：{error}")))?;
        let checkpoint_assistant_turn_id = tool_batch
            .checkpoint_assistant_turn_id()
            .map(str::to_string);
        let restored_provider_turn = hydrate_provider_continuation_history(
            &mut active_context,
            &llm_request.provider_profile_config,
            &llm_request.provider_protocol_key,
            trace_conversation_id.as_deref(),
            storage.as_deref(),
            provider_continuation_vault.as_deref(),
            ProviderContinuationResumeRequirement {
                required_refs: required_provider_continuation_refs.as_deref(),
                current_assistant_turn_id: checkpoint_assistant_turn_id.as_deref(),
            },
        )?;
        if let Some(restored_provider_turn) = restored_provider_turn.as_ref() {
            tool_batch.rehydrate_queued_calls_from_provider_turn(
                restored_provider_turn,
                |call| {
                    let projected = tool_registry.checkpoint_call_projection(&AgentToolCall {
                        id: call.id.clone(),
                        tool: call.name.clone(),
                        args: call.args.clone(),
                        approval_status: AgentApprovalStatus::NotRequired,
                        reason: None,
                    });
                    (
                        crate::llm::LlmToolCall {
                            id: projected.id,
                            name: projected.tool,
                            args: projected.args,
                        },
                        tool_registry.checkpoint_persistence(&call.name),
                    )
                },
            )?;
        }
        let mut settles_entire_provider_tool_batch_on_terminal = restored_provider_turn
            .as_ref()
            .map(|turn| {
                provider_runtime_capabilities
                    .classify_turn(
                        !turn.provider_tool_calls().is_empty(),
                        turn.provider_continuation().is_some(),
                        llm_request.provider_profile_config.reasoning.mode,
                    )
                    .settles_entire_batch_on_terminal()
            })
            .unwrap_or(false);
        if resumed_world_state_epoch {
            // The exact checkpoint context remains an immutable prefix. Resume establishes a new
            // run-state epoch only after the frozen pending Tool exchange has been closed, avoiding
            // any attempt to reconstruct authority from rendered context text.
            active_context.push(run_world_state.full_context_item()?);
        }
        let mut tool_failure_guard =
            ToolFailureGuard::from_trace(&conversation_trace.checkpoint_snapshot());
        let conversation_trace = Arc::new(Mutex::new(conversation_trace));
        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
        let capacity_detector = ContextCapacityDetector::for_model(
            &llm_request.model,
            llm_request.api_style,
            &llm_request.stable_tools,
        );
        let model_tool_result_gate = capacity_detector.model_tool_result_gate();
        let tool_output_budget = capacity_detector.text_budget(MODEL_TOOL_RESULT_MAX_TOKENS);
        let exact_history_storage = storage.clone();
        let mut tool_context = ToolExecutionContext::from_run_context(run_context.as_ref())
            .with_cancellation(cancellation_token.clone())
            .with_model_capabilities(model_capabilities)
            .with_runtime_services(run_id.clone(), storage)
            .with_file_observation_registry(file_observations)
            .with_file_change_tool_set_revision(effective_tool_set.revision().to_string())
            .with_file_change_provider_wire_revision(
                llm_request
                    .provider_protocol_key
                    .provider_configuration_revision
                    .clone()
                    .unwrap_or_else(|| {
                        crate::file_change::proposal_digest(&llm_request.provider_protocol_key)
                            .expect("validated ProviderProtocolKey serializes")
                    }),
            )
            .with_skill_resources(skill_resources)
            .with_command_runtime_profile_resolver(command_runtime_profile_resolver)
            .with_command_session_executor(command_session_executor)
            .with_agent_collaboration(
                agent_collaboration.clone(),
                trace_assistant_message_id.clone(),
            )
            .with_steer_input(steer_input.clone())
            .with_text_output_budget(tool_output_budget);
        event_stream.emit(state_event(
            &run_id,
            AgentRunStatus::Running,
            Some(run_id.clone()),
            None,
        ));
        let mut usage = None;
        let mut finish_reason = None;
        let mut response_fence_corrections = 0_usize;
        let mut empty_model_action_repair_pending = false;
        let mut can_drain_steer_input = false;
        let context_capacity_detector = context_window_configured.then_some(capacity_detector);
        let context_compaction_executor = context_compaction_services
            .map(ContextCompactionExecutor::new)
            .filter(|_| context_window_configured);
        if let Some(detector) = &context_capacity_detector {
            detector.prepare_frame(&mut active_context);
        }
        let mut pending_provider_continuation_handoff = None;
        let result: AgentResult<AgentChatOutput> = async {
            if cancellation_token.is_cancelled() {
                if settles_entire_provider_tool_batch_on_terminal && !tool_batch.is_empty()
                {
                    let mut pending_assistant_context =
                        take_pending_assistant_tool_context(&mut tool_batch)?;
                    settle_cancelled_grouped_tool_batch(
                        None,
                        &mut tool_batch,
                        &mut pending_assistant_context,
                        &mut active_context,
                        &conversation_trace,
                        trace_observer.as_ref(),
                        &mut event_stream,
                        tool_registry.as_ref(),
                        &model_tool_result_gate,
                        trace_assistant_message_id.as_deref(),
                        &run_id,
                    )?;
                }
                return Ok(cancelled_output(
                    run_id,
                    event_stream,
                    tool_definitions,
                    runtime_extensions.todo_state(),
                    usage,
                    finish_reason,
                ));
            }

            let final_content = 'agent_loop: loop {
                if cancellation_token.is_cancelled() {
                    if settles_entire_provider_tool_batch_on_terminal && !tool_batch.is_empty()
                    {
                        let mut pending_assistant_context =
                            take_pending_assistant_tool_context(&mut tool_batch)?;
                        settle_cancelled_grouped_tool_batch(
                            None,
                            &mut tool_batch,
                            &mut pending_assistant_context,
                            &mut active_context,
                            &conversation_trace,
                            trace_observer.as_ref(),
                            &mut event_stream,
                            tool_registry.as_ref(),
                            &model_tool_result_gate,
                            trace_assistant_message_id.as_deref(),
                            &run_id,
                        )?;
                    }
                    return Ok(cancelled_output(
                        run_id,
                        event_stream,
                        tool_definitions,
                        runtime_extensions.todo_state(),
                        usage,
                        finish_reason,
                    ));
                }
                if tool_batch.take_suppressed_narration() {
                    active_context.push(suppressed_narration_context_item());
                }
                if let Some(count) = tool_batch.take_deferred_external_tool_call_count() {
                    active_context.push(deferred_external_tool_calls_context_item(count));
                }
                if tool_batch.is_empty()
                    && next_model_request_index > self.max_tool_iterations
                    && !empty_model_action_repair_pending
                {
                    let message = "工具调用次数超过限制，已停止继续执行。".to_string();
                    let trace_sequence = conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_runtime_error(&message, false, None)
                        .map_err(AgentError::new)?;
                    publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    event_stream.emit(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        trace_sequence: Some(trace_sequence),
                        message: message.clone(),
                        recoverable: false,
                        code: None,
                        details: None,
                    });
                    return Err(AgentError::new(message));
                }
                if tool_batch.is_empty() {
                    active_context.validate_complete_tool_protocol()?;
                    if can_drain_steer_input {
                        if let Some(steer_input) = &steer_input {
                            let pending = steer_input.drain_pending();
                            if !pending.is_empty() {
                                apply_steer_inputs(
                                    &run_id,
                                    trace_assistant_message_id.as_deref(),
                                    None,
                                    pending,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    &mut tool_context,
                                    &mut run_context,
                                )?;
                            }
                        }
                    }
                    if let (Some(inbox), Some(conversation_id), Some(assistant_message_id)) = (
                        collaboration_inbox.as_ref(),
                        trace_conversation_id.as_deref(),
                        trace_assistant_message_id.as_deref(),
                    ) {
                        let expected_next_trace_sequence = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .next_sequence();
                        if let Some(delivery) = inbox.bind_for_model_batch(
                            AgentSamplingBoundaryRequest {
                                conversation_id: conversation_id.to_string(),
                                run_id: run_id.clone(),
                                assistant_message_id: assistant_message_id.to_string(),
                                model_batch_index: u64::try_from(
                                    next_model_request_index.saturating_add(1),
                                )
                                .unwrap_or(u64::MAX),
                                expected_next_trace_sequence,
                            },
                        )? {
                            apply_agent_mailbox_delivery(
                                &delivery,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                assistant_message_id,
                            )?;
                        }
                    }
                    let model_request_index = next_model_request_index;
                    next_model_request_index = next_model_request_index.saturating_add(1);
                    effective_tool_set = tool_registry.effective_tool_set(
                        permitted_tool_definitions.iter().cloned(),
                        &runtime_extensions.active_tool_capabilities()?,
                    )?;
                    tool_context.replace_file_change_tool_set_revision(
                        effective_tool_set.revision().to_string(),
                    );
                    if effective_tool_set.stable_revision() != stable_tool_revision {
                        return Err(AgentError::new(
                            "运行期间稳定工具前缀发生变化；为防止缓存和执行契约漂移，当前运行已停止。",
                        ));
                    }
                    tool_definitions = effective_tool_set.all_definitions();
                    if let Some(world_state_diff) = run_world_state.reconcile(
                        &effective_tool_set,
                        runtime_extensions.world_state_sections()?,
                        run_context.as_ref(),
                    )? {
                        active_context.push(world_state_diff);
                    }
                    if effective_tool_set.revision() != emitted_tool_set_revision {
                        event_stream.emit(AgentEvent::ToolSetChanged {
                            run_id: run_id.clone(),
                            stable_revision: effective_tool_set.stable_revision().to_string(),
                            dynamic_revision: effective_tool_set.dynamic_revision().to_string(),
                            effective_revision: effective_tool_set.revision().to_string(),
                            tool_definitions: tool_registry
                                .renderer_event_definitions(&tool_definitions),
                        });
                        emitted_tool_set_revision = effective_tool_set.revision().to_string();
                    }
                    let context_compaction_planner =
                        ContextCompactionPlanner::for_tools(&tool_definitions);
                    let file_transactions = FileTransactionState::load(
                        transaction_storage.as_deref(),
                        &run_id,
                        trace_conversation_id.as_deref(),
                        run_context
                            .as_ref()
                            .and_then(|context| context.project_id.as_deref()),
                    )?;
                    let user_text_blocked = file_transactions.blocks_user_text();
                    let mut compaction_attempts = 0_usize;
                    let (request_context, request_estimate) = loop {
                        let mut request_context = active_context.clone();
                        let mut request_estimate = None;
                        runtime_extensions.contribute_request_context(
                            &ModelRequestContext::agent_work(),
                            &mut request_context,
                        )?;
                        if empty_model_action_repair_pending {
                            request_context.push(empty_model_action_repair_context_item());
                        }
                        if let Some(context) = file_transactions.request_context() {
                            request_context.push(ContextItem::text(
                                LlmMessageRole::System,
                                context,
                                ContextSource::FileTransaction,
                                ContextScope::Run,
                                ContextRetention::RequestOnly,
                            ));
                        }
                        request_context.validate_cache_layout()?;
                        emit_context_manifest_if_enabled(
                            &run_id,
                            model_request_index + 1,
                            &request_context,
                            &tool_definitions,
                        );
                        let model_input_capacity = if let Some(detector) = &context_capacity_detector {
                            let report = detector.inspect_with_dynamic_tools(
                                &mut request_context,
                                llm_request.context_window_tokens,
                                llm_request.max_tokens,
                                effective_tool_set.dynamic_definitions(),
                            );
                            let compaction_query = report.compaction_query();
                            let compaction_plan = context_compaction_planner.plan(
                                    &compaction_query,
                                    &request_context.planning_items()?,
                                    true,
                                );
                            emit_context_budget_if_enabled(
                                &run_id,
                                model_request_index + 1,
                                &report,
                                &compaction_query,
                                &compaction_plan,
                            );
                            if compaction_attempts < MAX_CONTEXT_COMPACTION_ATTEMPTS_PER_REQUEST {
                                if let Some(executor) = &context_compaction_executor {
                                    let operation_id = format!(
                                        "{run_id}-context-compaction-{}-{}",
                                        model_request_index + 1,
                                        compaction_attempts + 1
                                    );
                                    let attempt = executor
                                        .begin(
                                            &compaction_plan,
                                            &operation_id,
                                            &run_id,
                                            trace_conversation_id.as_deref(),
                                            trace_assistant_message_id.as_deref(),
                                            u64::try_from(model_request_index + 1)
                                                .unwrap_or(u64::MAX),
                                            u64::try_from(compaction_attempts + 1)
                                                .unwrap_or(u64::MAX),
                                            &llm_request.model,
                                            llm_request.api_style,
                                            &cancellation_token,
                                        )
                                        .await?;
                                    if let Some(attempt) = attempt {
                                        let compaction_trace_sequence = conversation_trace
                                            .lock()
                                            .unwrap_or_else(|error| error.into_inner())
                                            .record_context_compaction_started(&operation_id)
                                            .map_err(AgentError::new)?;
                                        publish_trace_snapshot(
                                            &conversation_trace,
                                            trace_observer.as_ref(),
                                        )?;
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionStarted {
                                                run_id: run_id.clone(),
                                                operation_id: operation_id.clone(),
                                                trace_sequence: compaction_trace_sequence,
                                            },
                                        );
                                        let execution =
                                            executor.execute(attempt, &cancellation_token).await;
                                        let outcome = match &execution {
                                            Ok(ContextCompactionExecution::Applied { .. }) => {
                                                AgentContextCompactionEventOutcome::Applied
                                            }
                                            Ok(ContextCompactionExecution::Refreshed {
                                                ..
                                            }) => AgentContextCompactionEventOutcome::Skipped,
                                            Err(error) if error.is_cancelled() => {
                                                AgentContextCompactionEventOutcome::Cancelled
                                            }
                                            Err(_) => AgentContextCompactionEventOutcome::Failed,
                                        };
                                        let settled_trace_sequence = conversation_trace
                                            .lock()
                                            .unwrap_or_else(|error| error.into_inner())
                                            .record_context_compaction_finished(
                                                &operation_id,
                                                outcome,
                                            )
                                            .map_err(AgentError::new)?;
                                        debug_assert_eq!(
                                            settled_trace_sequence,
                                            compaction_trace_sequence
                                        );
                                        publish_trace_snapshot(
                                            &conversation_trace,
                                            trace_observer.as_ref(),
                                        )?;
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionFinished {
                                                run_id: run_id.clone(),
                                                operation_id,
                                                outcome,
                                                trace_sequence: settled_trace_sequence,
                                            },
                                        );
                                        match execution {
                                            Ok(ContextCompactionExecution::Applied {
                                                baseline,
                                                usage: compaction_usage,
                                            })
                                            | Ok(ContextCompactionExecution::Refreshed {
                                                baseline,
                                                usage: compaction_usage,
                                            }) => {
                                                merge_provider_usage(
                                                    &mut usage,
                                                    compaction_usage,
                                                    provider_runtime_capabilities.usage(),
                                                );
                                                active_context = (*baseline)
                                                    .replace_compacted_model_history(active_context);
                                                detector.prepare_frame(&mut active_context);
                                                publish_trace_snapshot(
                                                    &conversation_trace,
                                                    trace_observer.as_ref(),
                                                )?;
                                                compaction_attempts =
                                                    compaction_attempts.saturating_add(1);
                                                continue;
                                            }
                                            Err(error) if error.is_cancelled() => {
                                                merge_provider_usage(
                                                    &mut usage,
                                                    error.usage().cloned(),
                                                    provider_runtime_capabilities.usage(),
                                                );
                                                return Ok(cancelled_output(
                                                    run_id,
                                                    event_stream,
                                                    tool_definitions,
                                                    runtime_extensions.todo_state(),
                                                    usage,
                                                    finish_reason,
                                                ));
                                            }
                                            Err(error) => {
                                                merge_provider_usage(
                                                    &mut usage,
                                                    error.usage().cloned(),
                                                    provider_runtime_capabilities.usage(),
                                                );
                                                return Err(error.with_usage(usage));
                                            }
                                        }
                                    }
                                }
                            }
                            emit_compaction_skip_if_required(
                                &run_id,
                                model_request_index + 1,
                                compaction_attempts,
                                context_compaction_executor.is_some(),
                                &compaction_plan,
                            );
                            request_estimate =
                                Some(ModelRequestEstimate::from_budget_report(&report));
                            let context_window_snapshot = report.snapshot(&llm_request.model);
                            let remaining_tokens = report
                                .remaining_input_tokens
                                .map(|remaining| u64::try_from(remaining.max(0)).unwrap_or(0));
                            if context_window_indicator_enabled {
                                if let Some(observer) = context_window_observer.as_ref() {
                                    observer(context_window_snapshot);
                                }
                            }
                            // Publish the exact final request attempt before enforcing capacity so
                            // the Host can display an authoritative over-capacity state as well as
                            // sendable states. Earlier compaction attempts `continue` above and
                            // therefore never escape as misleading intermediate snapshots.
                            detector.ensure_sendable(report)?;
                            remaining_tokens.map(|remaining_tokens| ModelInputCapacity {
                                remaining_tokens,
                                text_budget: detector.text_budget(remaining_tokens.max(1)),
                                effective_tool_set: effective_tool_set.clone(),
                            })
                        } else {
                            None
                        };
                        request_context
                            .ensure_model_tool_results_fit(&model_tool_result_gate)?;
                        runtime_extensions.update_model_input_capacity(model_input_capacity);
                        break (request_context, request_estimate);
                    };
                    let observation_builder = ModelRequestObservationBuilder::new(
                        format!("model-request-{run_id}-agent-{}", model_request_index + 1),
                        run_id.clone(),
                        observation_conversation_id.clone(),
                        observation_assistant_message_id.clone(),
                        None,
                        u64::try_from(model_request_index + 1).unwrap_or(u64::MAX),
                        ModelRequestPurpose::AgentLoop,
                        llm_request.model.clone(),
                        llm_request.api_style,
                        request_estimate,
                        crate::storage::now_ms(),
                    )
                    .with_tool_set(ModelRequestToolSetObservation::new(
                        llm_request.api_style,
                        effective_tool_set.stable_revision(),
                        effective_tool_set.dynamic_revision(),
                        effective_tool_set.revision(),
                        u64::try_from(effective_tool_set.stable_definitions().len())
                            .unwrap_or(u64::MAX),
                        u64::try_from(effective_tool_set.dynamic_definitions().len())
                            .unwrap_or(u64::MAX),
                    ));
                    let request = llm_request.request(
                        request_context,
                        effective_tool_set.dynamic_definitions(),
                    );
                    let mut committed_message_stream_id = None;
                    let llm_response_result = if request.stream {
                        let delta_run_id = run_id.clone();
                        let stream_id = format!("{}-stream-{}", run_id, model_request_index + 1);
                        let delta_cancellation_token = cancellation_token.clone();
                        let mut tool_input_stream = ToolInputStreamObservers::default();
                        complete_chat_streaming(
                            request,
                            cancellation_token.clone(),
                            |stream_event| {
                                if delta_cancellation_token.is_cancelled() {
                                    return;
                                }
                                match stream_event {
                                    LlmStreamEvent::AttemptStarted {
                                        attempt,
                                        max_attempts,
                                    } => {
                                        tool_input_stream.start_attempt(attempt);
                                        if !user_text_blocked {
                                            let _ = max_attempts;
                                            event_stream.emit(AgentEvent::MessageStreamStarted {
                                                run_id: delta_run_id.clone(),
                                                stream_id: stream_id.clone(),
                                                attempt,
                                            });
                                        }
                                    }
                                    LlmStreamEvent::Delta(delta)
                                        if !user_text_blocked && !delta.is_empty() =>
                                    {
                                        event_stream.emit(AgentEvent::MessageDelta {
                                            run_id: delta_run_id.clone(),
                                            stream_id: Some(stream_id.clone()),
                                            delta,
                                        });
                                    }
                                    LlmStreamEvent::ToolInputProgress {
                                        tool_call_index,
                                        tool,
                                        input_delta,
                                        received_bytes,
                                    } => {
                                        // Stream fragments are provisional. Correlate them by
                                        // stream/attempt/index and leave Tool Call ID unset until
                                        // the complete response receives its canonical identity.
                                        let observation = tool_input_stream.on_delta(
                                            tool_registry.as_ref(),
                                            &tool_context,
                                            &stream_id,
                                            tool_call_index,
                                            None,
                                            &tool,
                                            &input_delta,
                                            received_bytes,
                                        );
                                        if observation.as_ref().is_ok_and(|value| !value.handled) {
                                            event_stream.emit_transient(
                                                AgentEvent::ToolInputProgress {
                                                    run_id: delta_run_id.clone(),
                                                    stream_id: stream_id.clone(),
                                                    attempt: tool_input_stream.attempt(),
                                                    tool_call_index,
                                                    tool_call_id: None,
                                                    tool: tool.clone(),
                                                    received_bytes,
                                                },
                                            );
                                        }
                                        if let Ok(observation) = observation {
                                            if let Some(preview) = observation.preview {
                                                emit_tool_input_preview(
                                                    &mut event_stream,
                                                    &delta_run_id,
                                                    preview,
                                                );
                                            }
                                        }
                                    }
                                    LlmStreamEvent::AttemptReset { reason } => {
                                        event_stream.emit_transient(
                                            AgentEvent::FileWritePreviewCleared {
                                                run_id: delta_run_id.clone(),
                                                stream_id: stream_id.clone(),
                                                attempt: tool_input_stream.attempt(),
                                            },
                                        );
                                        tool_input_stream.reset();
                                        if !user_text_blocked {
                                            event_stream.emit(AgentEvent::MessageStreamReset {
                                                run_id: delta_run_id.clone(),
                                                stream_id: stream_id.clone(),
                                                reason,
                                            });
                                        }
                                    }
                                    LlmStreamEvent::Retrying {
                                        attempt,
                                        max_attempts,
                                        category,
                                        provider_code,
                                        delay_ms,
                                        retry_at,
                                    } => {
                                        event_stream.emit(AgentEvent::LlmRetry {
                                            run_id: delta_run_id.clone(),
                                            stream_id: stream_id.clone(),
                                            attempt,
                                            max_attempts,
                                            category,
                                            provider_code,
                                            delay_ms,
                                            retry_at,
                                        });
                                    }
                                    LlmStreamEvent::Committed => {
                                        for preview in tool_input_stream.flush() {
                                            emit_tool_input_preview(
                                                &mut event_stream,
                                                &delta_run_id,
                                                preview,
                                            );
                                        }
                                        if !user_text_blocked {
                                            committed_message_stream_id = Some(stream_id.clone());
                                        }
                                    }
                                    LlmStreamEvent::Delta(_) => {}
                                }
                            },
                        )
                        .await
                    } else {
                        complete_chat(request, cancellation_token.clone()).await
                    };
                    let llm_response = match llm_response_result {
                        Ok(response) => {
                            let observation = observation_builder.completed(
                                response.usage.clone(),
                                response.finish_reason.clone(),
                                crate::storage::now_ms(),
                            )?;
                            if let Some(observer) = &model_request_observer {
                                observer(observation);
                            }
                            response
                        }
                        Err(error) => {
                            let observation = observation_builder.failed(
                                error.usage().cloned(),
                                &error,
                                crate::storage::now_ms(),
                            )?;
                            if let Some(observer) = &model_request_observer {
                                observer(observation);
                            }
                            if error.is_cancelled() {
                                merge_provider_usage(
                                    &mut usage,
                                    error.usage().cloned(),
                                    provider_runtime_capabilities.usage(),
                                );
                                return Ok(cancelled_output(
                                    run_id,
                                    event_stream,
                                    tool_definitions,
                                    runtime_extensions.todo_state(),
                                    usage,
                                    finish_reason,
                                ));
                            }
                            merge_provider_usage(
                                &mut usage,
                                error.usage().cloned(),
                                provider_runtime_capabilities.usage(),
                            );
                            if is_repairable_empty_model_action(&error)
                                && !empty_model_action_repair_pending
                            {
                                empty_model_action_repair_pending = true;
                                continue 'agent_loop;
                            }
                            return Err(error
                                .with_usage(usage)
                                .with_model_request_interruption());
                        }
                    };
                    empty_model_action_repair_pending = false;
                    let response_content = llm_response.content().to_string();
                    let provider_tool_calls = llm_response.provider_tool_calls().to_vec();
                    merge_provider_usage(
                        &mut usage,
                        llm_response.usage,
                        provider_runtime_capabilities.usage(),
                    );
                    finish_reason = llm_response.finish_reason;
                    let mut assistant_turn = llm_response.assistant_turn;
                    can_drain_steer_input = true;

                    let tool_bindings = tool_call_bindings_from_response(
                        provider_tool_calls,
                        &response_content,
                        &run_id,
                        model_request_index,
                    );
                    let tool_requests = tool_bindings
                        .iter()
                        .map(|binding| binding.runtime_call.clone())
                        .collect::<Vec<_>>();
                    let retained_assistant_content = if user_text_blocked {
                        ""
                    } else {
                        &response_content
                    };
                    let suppressed_narration =
                        user_text_blocked && !response_content.trim().is_empty();
                    if let Some(detector) = &context_capacity_detector {
                        let mut retained_tokens = detector
                            .estimate_assistant_tool_batch_tokens(
                                retained_assistant_content,
                                &tool_requests,
                            );
                        if suppressed_narration {
                            for message in ContextFrame::new(vec![
                                suppressed_narration_context_item(),
                            ])
                            .into_messages()
                            {
                                retained_tokens = retained_tokens.saturating_add(
                                    detector.estimate_message_tokens(&message),
                                );
                            }
                        }
                        runtime_extensions.consume_model_input_capacity(retained_tokens);
                    }
                    let committed_narration_sequence = if !tool_requests.is_empty()
                        && !user_text_blocked
                        && !response_content.trim().is_empty()
                    {
                        let sequence = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .record_narration(&response_content)
                            .map_err(AgentError::new)?;
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                        sequence
                    } else {
                        None
                    };
                    if let Some(stream_id) = committed_message_stream_id.take() {
                        event_stream.emit(AgentEvent::MessageStreamCommitted {
                            run_id: run_id.clone(),
                            stream_id,
                            trace_sequence: committed_narration_sequence,
                        });
                    }
                    if cancellation_token.is_cancelled() {
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
                    if user_text_blocked && tool_requests.is_empty() {
                        response_fence_corrections = response_fence_corrections.saturating_add(1);
                        if response_fence_corrections > MAX_RESPONSE_FENCE_CORRECTIONS {
                            return Err(AgentError::new(
                                "模型连续输出文字但未结算文件事务，已停止以避免循环。请重试任务。",
                            ));
                        }
                        active_context.push(ContextItem::text(
                            LlmMessageRole::System,
                            file_transactions.protocol_correction(),
                            ContextSource::RuntimeGuard,
                            ContextScope::Run,
                            ContextRetention::Retained,
                        ));
                        continue;
                    }
                    if tool_requests.is_empty() {
                        if let Some(steer_input) = &steer_input {
                            match steer_input.take_pending_or_close() {
                                AgentSteerDrainOrClose::Pending(pending) => {
                                    apply_steer_inputs(
                                        &run_id,
                                        trace_assistant_message_id.as_deref(),
                                        Some(&response_content),
                                        pending,
                                        &mut active_context,
                                        &conversation_trace,
                                        trace_observer.as_ref(),
                                        &mut event_stream,
                                        &mut tool_context,
                                        &mut run_context,
                                    )?;
                                    continue;
                                }
                                AgentSteerDrainOrClose::Closed => {}
                            }
                        }
                        break 'agent_loop response_content;
                    }
                    response_fence_corrections = 0;

                    if model_request_index >= self.max_tool_iterations {
                        let message = "工具调用次数超过限制，已停止继续执行。".to_string();
                        let trace_sequence = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .record_runtime_error(&message, false, None)
                            .map_err(AgentError::new)?;
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                        event_stream.emit(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            trace_sequence: Some(trace_sequence),
                            message: message.clone(),
                            recoverable: false,
                            code: None,
                            details: None,
                        });
                        return Err(AgentError::new(message));
                    }

                    assistant_turn.set_runtime_visible_text(retained_assistant_content);
                    if assistant_turn.provider_tool_calls().is_empty() && !tool_bindings.is_empty()
                    {
                        if provider_runtime_capabilities.requires_provider_native_tool_calls() {
                            return Err(AgentError::structured(
                                "provider_context_boundary_required",
                                "当前 Provider Profile 只能执行 Provider 原生 Tool Call，不能把文本猜测为工具协议。",
                                json!({
                                    "type": "providerContextBoundary",
                                    "recovery": "requestNativeProviderToolCalls"
                                }),
                            ));
                        }
                        let provider_protocol = assistant_turn
                            .provider_protocol()
                            .cloned()
                            .ok_or_else(|| {
                                AgentError::new(
                                    "Split-projection assistant turn 不能产生新的 text-fallback Tool Call。",
                                )
                            })?;
                        let fallback_provider_calls = tool_bindings
                            .iter()
                            .map(|binding| crate::llm::LlmToolCall {
                                id: binding.provider_call_id.clone(),
                                name: binding.runtime_call.name.clone(),
                                args: binding.runtime_call.args.clone(),
                            })
                            .collect();
                        assistant_turn = crate::llm::LlmAssistantTurn::from_provider(
                            provider_protocol,
                            assistant_turn.provider_visible_text(),
                            fallback_provider_calls,
                        )?;
                        assistant_turn.set_runtime_visible_text(retained_assistant_content);
                    }
                    let context_bindings = tool_bindings
                        .iter()
                        .map(|binding| -> AgentResult<_> {
                            let model_call = tool_registry.model_call_projection(&AgentToolCall {
                                id: binding.runtime_call.id.clone(),
                                tool: binding.runtime_call.name.clone(),
                                args: binding.runtime_call.args.clone(),
                                approval_status: AgentApprovalStatus::NotRequired,
                                reason: None,
                            });
                            let provider_call = assistant_turn
                                .provider_tool_calls()
                                .get(binding.provider_tool_index)
                                .ok_or_else(|| {
                                    AgentError::new(
                                        "Runtime Tool Call 映射引用了不存在的 Provider Tool Call。",
                                    )
                                })?;
                            Ok(crate::llm::LlmRuntimeToolCallBinding::new(
                                binding.provider_tool_index,
                                provider_call,
                                crate::llm::LlmToolCall {
                                    id: model_call.id,
                                    name: model_call.tool,
                                    args: model_call.args,
                                },
                            ))
                        })
                        .collect::<AgentResult<Vec<_>>>()?;
                    assistant_turn.set_runtime_tool_bindings(context_bindings)?;
                    let has_provider_continuation =
                        assistant_turn.provider_continuation().is_some();
                    let provider_turn_policy = provider_runtime_capabilities.classify_turn(
                        !assistant_turn.provider_tool_calls().is_empty(),
                        has_provider_continuation,
                        llm_request.provider_profile_config.reasoning.mode,
                    );
                    let continuation_requirement =
                        provider_turn_policy.continuation_requirement();
                    if matches!(
                        (continuation_requirement, has_provider_continuation),
                        (ProviderContinuationRequirement::Required, false)
                            | (ProviderContinuationRequirement::Forbidden, true)
                    ) {
                        return Err(provider_continuation_runtime_error(
                            crate::ProviderContinuationStoreError::InvalidTurn,
                        ));
                    }
                    settles_entire_provider_tool_batch_on_terminal =
                        provider_turn_policy.settles_entire_batch_on_terminal();
                    tool_batch = ToolCallBatch::from_provider_response(
                        &run_id,
                        model_request_index,
                        assistant_turn,
                        tool_bindings,
                        suppressed_narration,
                        |call| {
                            let projected =
                                tool_registry.checkpoint_call_projection(&AgentToolCall {
                                    id: call.id.clone(),
                                    tool: call.name.clone(),
                                    args: call.args.clone(),
                                    approval_status: AgentApprovalStatus::NotRequired,
                                    reason: None,
                                });
                            (
                                crate::llm::LlmToolCall {
                                    id: projected.id,
                                    name: projected.tool,
                                    args: projected.args,
                                },
                                tool_registry.checkpoint_persistence(&call.name),
                            )
                        },
                    )?;
                    if provider_turn_policy.requires_private_replay() {
                        let vault = provider_continuation_vault.as_ref().cloned().ok_or_else(|| {
                            provider_continuation_runtime_error(
                                crate::ProviderContinuationStoreError::CredentialUnavailable,
                            )
                        })?;
                        let conversation_id = trace_conversation_id.as_deref().ok_or_else(|| {
                            provider_continuation_runtime_error(
                                crate::ProviderContinuationStoreError::InvalidBinding,
                            )
                        })?;
                        let assistant_message_id = trace_assistant_message_id
                            .as_deref()
                            .ok_or_else(|| {
                                provider_continuation_runtime_error(
                                    crate::ProviderContinuationStoreError::InvalidBinding,
                                )
                            })?;
                        let request_index = u64::try_from(model_request_index).map_err(|_| {
                            provider_continuation_runtime_error(
                                crate::ProviderContinuationStoreError::InvalidBinding,
                            )
                        })?;
                        let persisted_turn = tool_batch.assistant_turn().ok_or_else(|| {
                            provider_continuation_runtime_error(
                                crate::ProviderContinuationStoreError::InvalidTurn,
                            )
                        })?;
                        let assistant_turn_id = persisted_turn.stable_id();
                        let assistant_turn_digest = persisted_turn.stable_digest();
                        let binding =
                            crate::provider_continuation_store::ProviderContinuationBinding {
                                conversation_id,
                                assistant_message_id,
                                run_id: &run_id,
                                request_index,
                                assistant_turn_id: &assistant_turn_id,
                                assistant_turn_digest: &assistant_turn_digest,
                                provider_protocol: &llm_request.provider_protocol_key,
                            };
                        let continuation_ref = vault
                            .persist_staged(binding, persisted_turn)
                            .map_err(provider_continuation_runtime_error)?
                            .ok_or_else(|| {
                                provider_continuation_runtime_error(
                                    crate::ProviderContinuationStoreError::PayloadNotFound,
                                )
                            })?;
                        if let Err(error) = tool_batch
                            .attach_provider_continuation_ref(continuation_ref.clone())
                        {
                            let _ = vault.release(
                                &continuation_ref,
                                crate::provider_continuation_store::ProviderContinuationOwner {
                                    conversation_id,
                                    assistant_message_id,
                                    run_id: &run_id,
                                },
                                now_ms(),
                            );
                            return Err(error);
                        }
                        pending_provider_continuation_handoff =
                            Some(PendingProviderContinuationHandoff {
                                vault,
                                continuation_ref,
                                conversation_id: conversation_id.to_string(),
                                assistant_message_id: assistant_message_id.to_string(),
                                run_id: run_id.clone(),
                                request_index,
                                assistant_turn_id,
                                assistant_turn_digest,
                                provider_protocol: llm_request.provider_protocol_key.clone(),
                            });
                    }
                }

                let mut pending_assistant_context =
                    match take_pending_assistant_tool_context(&mut tool_batch) {
                        Ok(context) => context,
                        Err(error) => {
                            release_pending_provider_continuation(
                                &mut pending_provider_continuation_handoff,
                            )?;
                            return Err(error);
                        }
                    };

                while let Some(queued_tool_call) = tool_batch.pop_front() {
                    if cancellation_token.is_cancelled() {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_cancelled_grouped_tool_batch(
                                Some(TerminalToolCallSettlement {
                                    queued: queued_tool_call,
                                    call: None,
                                    announced: false,
                                    dispatch_started: false,
                                    outcome: TerminalToolCallOutcome::Synthetic,
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                            promote_pending_provider_continuation(
                                &mut pending_provider_continuation_handoff,
                            )?;
                        }
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
                    let cancellation_queued_tool_call = queued_tool_call.clone();
                    let model_context_provider_identity =
                        cancellation_queued_tool_call.provider_identity()?;
                    let batch_claim = tool_batch.claim(&queued_tool_call.call);
                    let tool_exchange_group = queued_tool_call.context_group();
                    // Durable model context intentionally keeps the Generic adapter's historical
                    // one-assistant-per-call wire shape. The live Context owns one complete Turn,
                    // while this split projection is recorded at each call's own trace sequence.
                    let durable_trace_assistant_message = LlmMessage::assistant(
                        queued_tool_call.assistant_content.clone(),
                        vec![queued_tool_call.checkpoint_call.clone()],
                    );
                    let tool_request = queued_tool_call.call;
                    let reason = extract_reason_from_args(&tool_request.args);
                    // The effective definitions are both the model contract and the execution
                    // allowlist. The registry may retain tools hidden by the current permission
                    // mode; a hallucinated or text-fallback call must not resurrect one.
                    let tool_is_exposed = effective_tool_set.contains(&tool_request.name);
                    let definition_requires_approval = tool_is_exposed
                        && tool_registry
                            .requires_approval_for_call(&tool_request.name, &tool_request.args);
                    let mut call = AgentToolCall {
                        id: tool_request.id,
                        tool: tool_request.name,
                        args: tool_request.args,
                        approval_status: if definition_requires_approval {
                            AgentApprovalStatus::Required
                        } else {
                            AgentApprovalStatus::NotRequired
                        },
                        reason,
                    };
                    // Re-load at dispatch time instead of relying on the state observed before
                    // the model request. An earlier Tool Call in the same provider batch may have
                    // opened a transaction, and no later call may cross that newly-active fence.
                    let dispatch_file_transactions = FileTransactionState::load(
                        transaction_storage.as_deref(),
                        &run_id,
                        trace_conversation_id.as_deref(),
                        run_context
                            .as_ref()
                            .and_then(|context| context.project_id.as_deref()),
                    )?;
                    let file_transaction_fence_blocked = dispatch_file_transactions
                        .blocks_user_text()
                        && !dispatch_file_transactions.allows_tool_call(&call.tool, &call.args);
                    let is_policy_process_tool =
                        call.tool == "run_command" || call.tool == "skills_run_script";
                    let mut prepared_policy_action = None;
                    let mut policy_preflight_failure = None;
                    let mut terminate_after_repeat_guard_result = false;
                    let mut auto_execute_policy_action = false;
                    let mut requires_approval = definition_requires_approval;
                    let duplicate_in_batch = matches!(
                        batch_claim,
                        ToolCallBatchClaim::Duplicate { .. }
                    );
                    let tool_identity = tool_registry.identity(&call.tool).cloned().unwrap_or_else(
                        || AgentToolIdentity::Unregistered {
                            tool_name: call.tool.clone(),
                        },
                    );
                    let is_mcp_tool = matches!(&tool_identity, AgentToolIdentity::Mcp { .. });

                    if let ToolCallBatchClaim::Duplicate {
                        semantic_fingerprint,
                    } = batch_claim
                    {
                        let message = format!(
                            "Tool `{}` repeated the same semantic operation in one model response. \
                             The duplicate was not executed; use the result from the earlier call.",
                            call.tool
                        );
                        policy_preflight_failure = Some(AgentToolResult {
            exact_archive_file: None,
                            call_id: call.id.clone(),
                            tool: call.tool.clone(),
                            ok: false,
                            result: Some(json!({
                                "type": "runtime_guard",
                                "code": "duplicateToolCallInBatch",
                                "errorCode": "agent.duplicate_tool_call_in_batch",
                                "recovery": "useEarlierCallResult",
                                "tool": call.tool,
                                "semanticFingerprint": semantic_fingerprint,
                                "executed": false,
                                "message": message,
                            })),
                            error: Some(message),
                        });
                        requires_approval = false;
                        call.approval_status = AgentApprovalStatus::NotRequired;
                    }

                    if policy_preflight_failure.is_none() && file_transaction_fence_blocked {
                        let message = "FileChange transaction 尚未结算；当前调用已在副作用前拒绝。请使用 apply_patch 继续或结算返回的 exact transactionId。".to_string();
                        policy_preflight_failure = Some(AgentToolResult {
                            exact_archive_file: None,
                            call_id: call.id.clone(),
                            tool: call.tool.clone(),
                            ok: false,
                            result: Some(json!({
                                "type": "runtime_guard",
                                "code": "fileChangeTransactionUnsettled",
                                "errorCode": "agent.file_change_transaction_unsettled",
                                "recovery": "continueExactApplyPatchTransaction",
                                "executed": false,
                                "message": message,
                            })),
                            error: Some(message),
                        });
                        requires_approval = false;
                        call.approval_status = AgentApprovalStatus::NotRequired;
                    }

                    if policy_preflight_failure.is_none() {
                        if let Some(block) = tool_failure_guard.before_call(&call) {
                            terminate_after_repeat_guard_result = block.terminate_after_result;
                            policy_preflight_failure = Some(block.result);
                            requires_approval = false;
                            call.approval_status = AgentApprovalStatus::NotRequired;
                        }
                    }

                    if policy_preflight_failure.is_none() && !tool_is_exposed {
                        let unavailable_error =
                            unavailable_tool_error(&effective_tool_set, &call.tool);
                        policy_preflight_failure = Some(failed_tool_call_result(
                            &call,
                            unavailable_error,
                        ));
                    }

                    if policy_preflight_failure.is_none()
                        && is_policy_process_tool
                        && tool_is_exposed
                    {
                        match tool_registry
                            .proposed_action_async(&tool_context, &call)
                            .await
                        {
                            Ok(action) => match if call.tool == "run_command" {
                                prepare_command_dispatch(
                                    &call,
                                    action,
                                    command_permissions,
                                    command_workspace_root.as_deref(),
                                    command_auto_approve,
                                )
                            } else {
                                prepare_skill_script_dispatch(
                                    &call,
                                    action,
                                    command_permissions,
                                    command_workspace_root.as_deref(),
                                    command_auto_approve,
                                )
                            } {
                                CommandDispatch::ExecuteAutomatically(action) => {
                                    prepared_policy_action = Some(action);
                                    auto_execute_policy_action = true;
                                    requires_approval = false;
                                    call.approval_status = AgentApprovalStatus::Approved;
                                }
                                CommandDispatch::RequireApproval(action) => {
                                    prepared_policy_action = Some(action);
                                    requires_approval = true;
                                    call.approval_status = AgentApprovalStatus::Required;
                                }
                                CommandDispatch::Reject(result) => {
                                    policy_preflight_failure = Some(result);
                                    requires_approval = false;
                                    call.approval_status = AgentApprovalStatus::NotRequired;
                                }
                            },
                            Err(error) => {
                                policy_preflight_failure = Some(failed_tool_call_result(&call, error));
                                requires_approval = false;
                                call.approval_status = AgentApprovalStatus::NotRequired;
                            }
                        }
                    }

                    let uses_file_write_policy = tool_registry
                        .permission_policy(&call.tool)
                        .uses_file_write_approval();
                    if !is_policy_process_tool
                        && policy_preflight_failure.is_none()
                        && uses_file_write_policy
                        && definition_requires_approval
                        && file_write_approval_route(command_permissions)
                            == FileWriteApprovalRoute::Denied
                    {
                        policy_preflight_failure = Some(failed_tool_call_result(
                            &call,
                            AgentError::structured(
                                "agent.file_write_permission_denied",
                                "The current permission policy does not allow file changes.",
                                json!({
                                    "type": "file_write_policy",
                                    "code": "writePermissionDenied",
                                    "recovery": "changePermissions",
                                }),
                            ),
                        ));
                        requires_approval = false;
                    }
                    let auto_execute_patch = policy_preflight_failure.is_none()
                        && uses_file_write_policy
                        && patch_auto_approve
                        && definition_requires_approval;
                    let auto_execute_mcp_action = policy_preflight_failure.is_none()
                        && tool_registry.auto_executes_prepared_action(&call.tool);
                    // Built-in capability actions still need their typed Host preparation even
                    // when the effective permission skips the human prompt. In particular,
                    // activation mints a run-bound capability grant and sensitive MCP calls mint
                    // a target-bound one-shot grant. Only calls whose reviewed call-level policy
                    // actually requires approval enter this route, so Dynamic tools with benign
                    // arguments continue through the ordinary direct execution path.
                    let auto_execute_builtin_action = policy_preflight_failure.is_none()
                        && auto_executes_builtin_prepared_action(
                            command_permissions.builtin_execution,
                            definition_requires_approval,
                            &tool_identity,
                        );
                    let auto_execute_host_action = auto_execute_policy_action
                        || auto_execute_patch
                        || auto_execute_mcp_action
                        || auto_execute_builtin_action;
                    if !is_policy_process_tool {
                        if policy_preflight_failure.is_some() {
                            // A rejected or unavailable call is terminal for this attempt. Never
                            // turn a policy failure back into a pending approval merely because
                            // the underlying tool normally writes files.
                            requires_approval = false;
                            call.approval_status = AgentApprovalStatus::NotRequired;
                        } else {
                            requires_approval =
                                definition_requires_approval && !auto_execute_host_action;
                            call.approval_status = if auto_execute_host_action {
                                AgentApprovalStatus::Approved
                            } else if requires_approval {
                                AgentApprovalStatus::Required
                            } else {
                                AgentApprovalStatus::NotRequired
                            };
                        }
                    }
                    if pending_assistant_context
                        .as_ref()
                        .is_some_and(|(_, _, batch_group)| batch_group != &tool_exchange_group)
                    {
                        let error = AgentError::new(
                            "Tool Call 批次的 Assistant Turn 与结果分组不一致。",
                        );
                        release_pending_provider_continuation(
                            &mut pending_provider_continuation_handoff,
                        )?;
                        return Err(error);
                    }
                    let trace_call = tool_registry.trace_call_projection(&call);
                    let checkpoint_call = tool_registry.checkpoint_call_projection(&call);
                    let call_sequence = {
                        let mut recorder = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let sequence = recorder
                            .record_tool_call_with_identity(&trace_call, tool_identity.clone());
                        if let Some(sequence) = sequence {
                            recorder
                                .record_model_tool_call_message(
                                sequence,
                                0,
                                &durable_trace_assistant_message,
                                model_context_provider_identity.clone(),
                            )
                                .map_err(AgentError::new)?;
                            if trace_call.args != call.args || checkpoint_call.args != call.args {
                                recorder.mark_truncated();
                            }
                        }
                        sequence
                    };
                    if let Some((live_message, checkpoint_message, batch_group)) =
                        pending_assistant_context.take()
                    {
                        debug_assert_eq!(batch_group, tool_exchange_group);
                        active_context.push(
                            ContextItem::new(
                                live_message,
                                with_trace_origin(
                                    ContextMetadata::new(
                                        ContextSource::ModelResponse,
                                        ContextScope::Run,
                                        ContextRetention::Retained,
                                    )
                                    .with_group(batch_group),
                                    trace_assistant_message_id.as_deref(),
                                    call_sequence,
                                ),
                            )
                            .with_checkpoint_message(checkpoint_message),
                        );
                    }
                    if let Err(error) =
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())
                    {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_aborted_grouped_tool_batch(
                                &error,
                                Some(TerminalToolCallSettlement {
                                    queued: cancellation_queued_tool_call.clone(),
                                    call: Some(call.clone()),
                                    announced: false,
                                    dispatch_started: false,
                                    outcome: TerminalToolCallOutcome::Synthetic,
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                None,
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Err(error);
                    }
                    if let Err(error) = promote_pending_provider_continuation(
                        &mut pending_provider_continuation_handoff,
                    ) {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_aborted_grouped_tool_batch(
                                &error,
                                Some(TerminalToolCallSettlement {
                                    queued: cancellation_queued_tool_call.clone(),
                                    call: Some(call.clone()),
                                    announced: false,
                                    dispatch_started: false,
                                    outcome: TerminalToolCallOutcome::Synthetic,
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                            promote_pending_provider_continuation(
                                &mut pending_provider_continuation_handoff,
                            )?;
                        }
                        return Err(error);
                    }
                    if !is_mcp_tool {
                        let event_call = tool_registry.event_call_projection(&call);
                        event_stream.emit(AgentEvent::ToolCall {
                            run_id: run_id.clone(),
                            trace_sequence: call_sequence
                                .expect("a ToolCall always has a durable trace sequence"),
                            call: event_call,
                            identity: tool_identity.clone(),
                        });
                    }
                    if cancellation_token.is_cancelled() {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_cancelled_grouped_tool_batch(
                                Some(TerminalToolCallSettlement {
                                    queued: cancellation_queued_tool_call.clone(),
                                    call: Some(call.clone()),
                                    announced: true,
                                    dispatch_started: false,
                                    outcome: TerminalToolCallOutcome::Synthetic,
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }

                    if requires_approval {
                        let action_result = if is_policy_process_tool {
                            prepared_policy_action.take().ok_or_else(|| {
                                AgentError::new(format!(
                                    "{} lost its validated action snapshot before approval.",
                                    call.tool
                                ))
                            })
                        } else {
                            tool_registry
                                .proposed_action_async(&tool_context, &call)
                                .await
                        };
                        let action = match action_result {
                            Ok(action) => {
                                conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .enrich_tool_call(&action);
                                if let Err(error) = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                ) {
                                    let _ = tool_registry.invalidate_proposed_action(&action);
                                    if settles_entire_provider_tool_batch_on_terminal {
                                        settle_aborted_grouped_tool_batch(
                                            &error,
                                            Some(TerminalToolCallSettlement {
                                                queued: cancellation_queued_tool_call.clone(),
                                                call: Some(call.clone()),
                                                announced: true,
                                                dispatch_started: false,
                                                outcome: TerminalToolCallOutcome::Synthetic,
                                            }),
                                            &mut tool_batch,
                                            &mut pending_assistant_context,
                                            &mut active_context,
                                            &conversation_trace,
                                            None,
                                            &mut event_stream,
                                            tool_registry.as_ref(),
                                            &model_tool_result_gate,
                                            trace_assistant_message_id.as_deref(),
                                            &run_id,
                                        )?;
                                    }
                                    return Err(error);
                                }
                                action
                            }
                            Err(error) => {
                                let result = failed_tool_call_result(&call, error);
                                tool_failure_guard.observe(&call, &result);
                                let llm_result = tool_registry.model_projection(&result);
                                let checkpoint_result =
                                    tool_registry.checkpoint_projection(&result);
                                let result_sequence = conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .pending_tool_result_sequence(&call.id);
                                let archive_result = tool_registry.archive_projection(&result);
                                let archive_metadata =
                                    if tool_registry.archives_result(&call.tool) {
                                        archive_tool_result(ToolResultArchiveRequest {
                                            storage: exact_history_storage.as_ref(),
                                            conversation_id: trace_conversation_id.as_deref(),
                                            assistant_message_id:
                                                trace_assistant_message_id.as_deref(),
                                            sequence: result_sequence,
                                            raw_result: &result,
                                            archive_result: &archive_result,
                                            model_result: &llm_result,
                                            model_tool_result_gate: &model_tool_result_gate,
                                        })?
                                    } else {
                                        ConversationHistoryArchiveTraceMetadata::default()
                                    };
                                let model_observation = match finalize_model_tool_observation(
                                    &model_tool_result_gate,
                                    &call.id,
                                    true,
                                    &llm_result,
                                    &archive_metadata,
                                ) {
                                    Ok(observation) => observation,
                                    Err(error) => {
                                        if settles_entire_provider_tool_batch_on_terminal {
                                            settle_aborted_grouped_tool_batch(
                                                &error,
                                                Some(TerminalToolCallSettlement {
                                                    queued: cancellation_queued_tool_call.clone(),
                                                    call: Some(call.clone()),
                                                    announced: true,
                                                    dispatch_started: false,
                                                    outcome: TerminalToolCallOutcome::Synthetic,
                                                }),
                                                &mut tool_batch,
                                                &mut pending_assistant_context,
                                                &mut active_context,
                                                &conversation_trace,
                                                trace_observer.as_ref(),
                                                &mut event_stream,
                                                tool_registry.as_ref(),
                                                &model_tool_result_gate,
                                                trace_assistant_message_id.as_deref(),
                                                &run_id,
                                            )?;
                                        }
                                        return Err(error);
                                    }
                                };
                                let recorded_result_sequence = {
                                    let mut recorder = conversation_trace
                                        .lock()
                                        .unwrap_or_else(|error| error.into_inner());
                                    let sequence = recorder.record_tool_result_with_archive(
                                        &call,
                                        &checkpoint_result,
                                        archive_metadata.clone(),
                                    );
                                    if let Some(sequence) = sequence {
                                        recorder
                                            .record_model_message(
                                            sequence,
                                            0,
                                            &LlmMessage::tool_result(
                                                call.id.clone(),
                                                model_observation.clone(),
                                                true,
                                            ),
                                        )
                                            .map_err(AgentError::new)?;
                                    }
                                    sequence
                                };
                                if let Err(error) = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                ) {
                                    if settles_entire_provider_tool_batch_on_terminal {
                                        settle_aborted_grouped_tool_batch(
                                            &error,
                                            Some(TerminalToolCallSettlement {
                                                queued: cancellation_queued_tool_call.clone(),
                                                call: Some(call.clone()),
                                                announced: true,
                                                dispatch_started: false,
                                                outcome: TerminalToolCallOutcome::Settled(Box::new(
                                                    SettledTerminalToolCallOutcome {
                                                        result: result.clone(),
                                                        checkpoint_result:
                                                            checkpoint_result.clone(),
                                                        model_observation:
                                                            model_observation.clone(),
                                                        checkpoint_observation:
                                                            model_observation.clone(),
                                                        archive_metadata:
                                                            archive_metadata.clone(),
                                                    },
                                                )),
                                            }),
                                            &mut tool_batch,
                                            &mut pending_assistant_context,
                                            &mut active_context,
                                            &conversation_trace,
                                            None,
                                            &mut event_stream,
                                            tool_registry.as_ref(),
                                            &model_tool_result_gate,
                                            trace_assistant_message_id.as_deref(),
                                            &run_id,
                                        )?;
                                    }
                                    return Err(error);
                                }
                                if !is_mcp_tool {
                                    event_stream.emit(AgentEvent::ToolResult {
                                        run_id: run_id.clone(),
                                        result: redact_tool_result_for_event(
                                            &tool_registry.event_projection(&result),
                                        ),
                                    });
                                }
                                active_context.push(ContextItem::tool_result(
                                    call.id.clone(),
                                    model_observation,
                                    true,
                                    with_trace_origin(
                                        ContextMetadata::new(
                                            ContextSource::ToolResult,
                                            ContextScope::Run,
                                            ContextRetention::Retained,
                                        )
                                        .with_group(tool_exchange_group.clone()),
                                        trace_assistant_message_id.as_deref(),
                                        recorded_result_sequence,
                                    ),
                                ));
                                continue;
                            }
                        };
                        if cancellation_token.is_cancelled() {
                            let _ = tool_registry.invalidate_proposed_action(&action);
                            if settles_entire_provider_tool_batch_on_terminal {
                                settle_cancelled_grouped_tool_batch(
                                    Some(TerminalToolCallSettlement {
                                        queued: cancellation_queued_tool_call.clone(),
                                        call: Some(call.clone()),
                                        announced: true,
                                        dispatch_started: false,
                                        outcome: TerminalToolCallOutcome::Synthetic,
                                    }),
                                    &mut tool_batch,
                                    &mut pending_assistant_context,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    tool_registry.as_ref(),
                                    &model_tool_result_gate,
                                    trace_assistant_message_id.as_deref(),
                                    &run_id,
                                )?;
                            }
                            return Ok(cancelled_output(
                                run_id,
                                event_stream,
                                tool_definitions,
                                runtime_extensions.todo_state(),
                                usage,
                                finish_reason,
                            ));
                        }
                        if let AgentProposedAction::Diff { diff } = &action {
                            event_stream.emit(AgentEvent::Diff {
                                run_id: run_id.clone(),
                                diff: diff.clone(),
                            });
                        }
                        if matches!(
                            &action,
                            AgentProposedAction::McpToolCall { .. }
                                | AgentProposedAction::BuiltinMcpToolApproval { .. }
                        )
                            && !provider_runtime_capabilities
                                .allows_encrypted_checkpoint_rehydration()
                        {
                            let deferred_calls = tool_batch.defer_external_calls(|queued| {
                                queued.checkpoint_persistence
                                    != crate::tools::AgentToolCallCheckpointPersistence::Allowed
                                    || queued.checkpoint_call.args != queued.call.args
                            });
                            let deferred_call_ids = deferred_calls
                                .into_iter()
                                .map(|deferred| deferred.call.id)
                                .collect::<std::collections::BTreeSet<_>>();
                            active_context.omit_runtime_tool_calls_from_group(
                                &tool_exchange_group,
                                &deferred_call_ids,
                            )?;
                            conversation_trace
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .omit_model_tool_calls(&deferred_call_ids);
                            publish_trace_snapshot(
                                &conversation_trace,
                                trace_observer.as_ref(),
                            )?;
                        }
                        let extension_snapshots = match runtime_extensions.snapshots() {
                            Ok(snapshots) => snapshots,
                            Err(error) => {
                                let _ = tool_registry.invalidate_proposed_action(&action);
                                if settles_entire_provider_tool_batch_on_terminal {
                                    settle_aborted_grouped_tool_batch(
                                        &error,
                                        Some(TerminalToolCallSettlement {
                                            queued: cancellation_queued_tool_call.clone(),
                                            call: Some(call.clone()),
                                            announced: true,
                                            dispatch_started: false,
                                            outcome: TerminalToolCallOutcome::Synthetic,
                                        }),
                                        &mut tool_batch,
                                        &mut pending_assistant_context,
                                        &mut active_context,
                                        &conversation_trace,
                                        trace_observer.as_ref(),
                                        &mut event_stream,
                                        tool_registry.as_ref(),
                                        &model_tool_result_gate,
                                        trace_assistant_message_id.as_deref(),
                                        &run_id,
                                    )?;
                                }
                                return Err(error);
                            }
                        };
                        let checkpoint_result = {
                            let trace = conversation_trace
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            create_run_checkpoint_with_file_observations(
                                &run_id,
                                RunCheckpointState {
                                    context: &active_context,
                                    next_model_request_index,
                                    tool_batch: &tool_batch,
                                    extension_snapshots,
                                    pending_tool_call_id: &call.id,
                                    conversation_trace: &trace,
                                    tool_set: &effective_tool_set,
                                    run_context: run_context.as_ref(),
                                    collaboration_run_snapshot: agent_collaboration
                                        .as_ref()
                                        .map(AgentCollaborationRuntimeServices::run_snapshot),
                                    model_capabilities,
                                    run_world_state: run_world_state.snapshot(),
                                    provider_profile_config: &llm_request.provider_profile_config,
                                    provider_protocol_key: &llm_request.provider_protocol_key,
                                },
                                tool_context.file_observation_registry(),
                            )
                        };
                        let mut checkpoint = match checkpoint_result {
                            Ok(checkpoint) => checkpoint,
                            Err(error) => {
                                let _ = tool_registry.invalidate_proposed_action(&action);
                                if settles_entire_provider_tool_batch_on_terminal {
                                    settle_aborted_grouped_tool_batch(
                                        &error,
                                        Some(TerminalToolCallSettlement {
                                            queued: cancellation_queued_tool_call.clone(),
                                            call: Some(call.clone()),
                                            announced: true,
                                            dispatch_started: false,
                                            outcome: TerminalToolCallOutcome::Synthetic,
                                        }),
                                        &mut tool_batch,
                                        &mut pending_assistant_context,
                                        &mut active_context,
                                        &conversation_trace,
                                        trace_observer.as_ref(),
                                        &mut event_stream,
                                        tool_registry.as_ref(),
                                        &model_tool_result_gate,
                                        trace_assistant_message_id.as_deref(),
                                        &run_id,
                                    )?;
                                }
                                return Err(error);
                            }
                        };
                        if cancellation_token.is_cancelled() {
                            let _ = tool_registry.invalidate_proposed_action(&action);
                            if settles_entire_provider_tool_batch_on_terminal {
                                settle_cancelled_grouped_tool_batch(
                                    Some(TerminalToolCallSettlement {
                                        queued: cancellation_queued_tool_call.clone(),
                                        call: Some(call.clone()),
                                        announced: true,
                                        dispatch_started: false,
                                        outcome: TerminalToolCallOutcome::Synthetic,
                                    }),
                                    &mut tool_batch,
                                    &mut pending_assistant_context,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    tool_registry.as_ref(),
                                    &model_tool_result_gate,
                                    trace_assistant_message_id.as_deref(),
                                    &run_id,
                                )?;
                            }
                            return Ok(cancelled_output(
                                run_id,
                                event_stream,
                                tool_definitions,
                                runtime_extensions.todo_state(),
                                usage,
                                finish_reason,
                            ));
                        }
                        if let AgentProposedAction::McpToolCall { approval } = &action {
                            checkpoint.pending_action_id =
                                Some(approval.identity.action_id.clone());
                            let invocation = match crate::tools::mcp_tool_invocation_event(
                                approval,
                                crate::tools::McpToolInvocationEventUpdate {
                                    state: crate::protocol::AgentMcpToolInvocationState::PendingApproval,
                                    dispatch_certainty: crate::protocol::AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                                    outcome: None,
                                    is_error: None,
                                    error_code: None,
                                    duration_ms: None,
                                    output_truncated: false,
                                    result_size: None,
                                    failure_stage: None,
                                },
                            ) {
                                Ok(invocation) => invocation,
                                Err(error) => {
                                    let _ = tool_registry.invalidate_proposed_action(&action);
                                    if settles_entire_provider_tool_batch_on_terminal {
                                        settle_aborted_grouped_tool_batch(
                                            &error,
                                            Some(TerminalToolCallSettlement {
                                                queued: cancellation_queued_tool_call.clone(),
                                                call: Some(call.clone()),
                                                announced: true,
                                                dispatch_started: false,
                                                outcome: TerminalToolCallOutcome::Synthetic,
                                            }),
                                            &mut tool_batch,
                                            &mut pending_assistant_context,
                                            &mut active_context,
                                            &conversation_trace,
                                            trace_observer.as_ref(),
                                            &mut event_stream,
                                            tool_registry.as_ref(),
                                            &model_tool_result_gate,
                                            trace_assistant_message_id.as_deref(),
                                            &run_id,
                                        )?;
                                    }
                                    return Err(error);
                                }
                            };
                            event_stream.emit(AgentEvent::McpToolInvocationStateChanged {
                                run_id: run_id.clone(),
                                invocation,
                            });
                        } else if let AgentProposedAction::BuiltinCapabilityActivation {
                            approval,
                        } = &action
                        {
                            checkpoint.pending_action_id = Some(approval.action_id.clone());
                        } else if let AgentProposedAction::BuiltinMcpToolApproval { approval } =
                            &action
                        {
                            checkpoint.pending_action_id =
                                Some(approval.identity.action_id.clone());
                        }
                        event_stream.emit(AgentEvent::ApprovalRequired {
                            run_id: run_id.clone(),
                            action: Box::new(action.clone()),
                            checkpoint: Box::new(checkpoint),
                        });
                        event_stream.emit(state_event(
                            &run_id,
                            AgentRunStatus::WaitingForApproval,
                            Some(run_id.clone()),
                            None,
                        ));
                        event_stream.emit(done_event(
                            &run_id,
                            true,
                            AgentRunStatus::WaitingForApproval,
                            None,
                            usage.clone(),
                            finish_reason.clone(),
                            vec![action.clone()],
                        ));
                        file_transaction_guard.preserve_for_approval();

                        return Ok(AgentChatOutput {
                            content: String::new(),
                            status: AgentRunStatus::WaitingForApproval,
                            run_id,
                            events: event_stream.into_events(),
                            tool_definitions,
                            todo: runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                            proposed_actions: vec![action],
                            conversation_turn_trace: None,
                        });
                    }

                    let mut tool_result_persistence =
                        crate::tools::AgentToolResultPersistence::RuntimeCommits;
                    let result_result = if let Some(result) = policy_preflight_failure {
                        Ok(result)
                    } else if auto_execute_host_action {
                        let action_result = if is_policy_process_tool {
                            prepared_policy_action.take().ok_or_else(|| {
                                AgentError::new(format!(
                                    "{} lost its validated action snapshot before automatic execution.",
                                    call.tool
                                ))
                            })
                        } else {
                            tool_registry
                                .proposed_action_async(&tool_context, &call)
                                .await
                        };
                        match action_result {
                            Ok(action) => {
                                let action = approve_proposed_action(action);
                                conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .enrich_tool_call(&action);
                                if let Err(error) = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                ) {
                                    if settles_entire_provider_tool_batch_on_terminal {
                                        settle_aborted_grouped_tool_batch(
                                            &error,
                                            Some(TerminalToolCallSettlement {
                                                queued: cancellation_queued_tool_call.clone(),
                                                call: Some(call.clone()),
                                                announced: true,
                                                dispatch_started: false,
                                                outcome: TerminalToolCallOutcome::Synthetic,
                                            }),
                                            &mut tool_batch,
                                            &mut pending_assistant_context,
                                            &mut active_context,
                                            &conversation_trace,
                                            None,
                                            &mut event_stream,
                                            tool_registry.as_ref(),
                                            &model_tool_result_gate,
                                            trace_assistant_message_id.as_deref(),
                                            &run_id,
                                        )?;
                                    }
                                    return Err(error);
                                }
                                if let AgentProposedAction::Diff { diff } = &action {
                                    event_stream.emit(AgentEvent::Diff {
                                        run_id: run_id.clone(),
                                        diff: diff.clone(),
                                    });
                                }
                                let frozen_pending_action_id = match &action {
                                    AgentProposedAction::McpToolCall { approval } => {
                                        Some(approval.identity.action_id.clone())
                                    }
                                    AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                                        Some(approval.identity.action_id.clone())
                                    }
                                    _ => None,
                                };
                                let frozen_checkpoint_result =
                                    if let Some(pending_action_id) = frozen_pending_action_id {
                                        runtime_extensions.snapshots().and_then(
                                            |extension_snapshots| {
                                                let trace = conversation_trace
                                                    .lock()
                                                    .unwrap_or_else(|error| error.into_inner());
                                                create_run_checkpoint_with_file_observations(
                                                    &run_id,
                                                    RunCheckpointState {
                                                        context: &active_context,
                                                        next_model_request_index,
                                                        tool_batch: &tool_batch,
                                                        extension_snapshots,
                                                        pending_tool_call_id: &call.id,
                                                        conversation_trace: &trace,
                                                        tool_set: &effective_tool_set,
                                                        run_context: run_context.as_ref(),
                                                        collaboration_run_snapshot:
                                                            agent_collaboration.as_ref().map(
                                                                AgentCollaborationRuntimeServices::run_snapshot,
                                                            ),
                                                        model_capabilities,
                                                        run_world_state: run_world_state.snapshot(),
                                                        provider_profile_config:
                                                            &llm_request.provider_profile_config,
                                                        provider_protocol_key:
                                                            &llm_request.provider_protocol_key,
                                                    },
                                                    tool_context.file_observation_registry(),
                                                )
                                                .map(|mut checkpoint| {
                                                    checkpoint.pending_action_id =
                                                        Some(pending_action_id);
                                                    checkpoint
                                                })
                                            },
                                        )
                                        .map(Some)
                                    } else {
                                        Ok(None)
                                    };
                                match frozen_checkpoint_result {
                                    Ok(frozen_checkpoint) => {
                                        if let Some(executor) = host_executor.as_ref() {
                                            execute_host_action_on_blocking_thread(
                                                executor.clone(),
                                                action,
                                                call.clone(),
                                                frozen_checkpoint,
                                                cancellation_token.clone(),
                                            )
                                            .await
                                        } else {
                                            let _ =
                                                tool_registry.invalidate_proposed_action(&action);
                                            Ok(failed_tool_call_result(
                                                &call,
                                                AgentError::structured(
                                                    "agent.host_executor_unavailable",
                                                    "The Host execution boundary is unavailable.",
                                                    json!({
                                                        "type": "host_execution",
                                                        "code": "hostExecutorUnavailable",
                                                        "outcome": "not_dispatched",
                                                        "retryable": false,
                                                    }),
                                                ),
                                            ))
                                        }
                                    }
                                    Err(error) => {
                                        let _ = tool_registry.invalidate_proposed_action(&action);
                                        Ok(failed_tool_call_result(&call, error))
                                    }
                                }
                            }
                            Err(error) => Ok(failed_tool_call_result(&call, error)),
                        }
                    } else {
                        execute_registered_tool(
                            tool_registry.clone(),
                            tool_context.clone().with_model_batch_index(
                                u64::try_from(next_model_request_index.saturating_add(1))
                                    .unwrap_or(u64::MAX),
                            ),
                            call.clone(),
                            cancellation_token.clone(),
                        )
                        .await
                        .map(|execution| {
                            tool_result_persistence = execution.persistence;
                            execution.result
                        })
                    };
                    let authoritative_tool_settlement = matches!(
                        tool_registry.cancellation_settlement(&call.tool),
                        crate::tools::AgentToolCancellationSettlement::Authoritative
                    );
                    let result = match result_result {
                        Ok(result) => result,
                        Err(error) if error.is_cancelled() => {
                            if settles_entire_provider_tool_batch_on_terminal {
                                settle_cancelled_grouped_tool_batch(
                                    Some(TerminalToolCallSettlement {
                                        queued: cancellation_queued_tool_call.clone(),
                                        call: Some(call.clone()),
                                        announced: true,
                                        dispatch_started: true,
                                        outcome: TerminalToolCallOutcome::Synthetic,
                                    }),
                                    &mut tool_batch,
                                    &mut pending_assistant_context,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    tool_registry.as_ref(),
                                    &model_tool_result_gate,
                                    trace_assistant_message_id.as_deref(),
                                    &run_id,
                                )?;
                            }
                            return Ok(cancelled_output(
                                run_id,
                                event_stream,
                                tool_definitions,
                                runtime_extensions.todo_state(),
                                usage,
                                finish_reason,
                            ));
                        }
                        Err(error) => {
                            if settles_entire_provider_tool_batch_on_terminal {
                                let failed_result = failed_tool_call_result(&call, error.clone());
                                settle_aborted_grouped_tool_batch(
                                    &error,
                                    Some(TerminalToolCallSettlement {
                                        queued: cancellation_queued_tool_call.clone(),
                                        call: Some(call.clone()),
                                        announced: true,
                                        dispatch_started: true,
                                        outcome: TerminalToolCallOutcome::Authoritative(
                                            failed_result,
                                        ),
                                    }),
                                    &mut tool_batch,
                                    &mut pending_assistant_context,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    tool_registry.as_ref(),
                                    &model_tool_result_gate,
                                    trace_assistant_message_id.as_deref(),
                                    &run_id,
                                )?;
                            }
                            return Err(error);
                        }
                    };
                    if settles_entire_provider_tool_batch_on_terminal
                        && cancellation_preempts_tool_result(
                            auto_execute_host_action,
                            if authoritative_tool_settlement {
                                crate::tools::AgentToolCancellationSettlement::Authoritative
                            } else {
                                crate::tools::AgentToolCancellationSettlement::Interruptible
                            },
                            cancellation_token.is_cancelled(),
                            &result,
                        )
                    {
                        settle_cancelled_grouped_tool_batch(
                            Some(TerminalToolCallSettlement {
                                queued: cancellation_queued_tool_call.clone(),
                                call: Some(call.clone()),
                                announced: true,
                                dispatch_started: true,
                                outcome: TerminalToolCallOutcome::Synthetic,
                            }),
                            &mut tool_batch,
                            &mut pending_assistant_context,
                            &mut active_context,
                            &conversation_trace,
                            trace_observer.as_ref(),
                            &mut event_stream,
                            tool_registry.as_ref(),
                            &model_tool_result_gate,
                            trace_assistant_message_id.as_deref(),
                            &run_id,
                        )?;
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
                    if !duplicate_in_batch {
                        tool_failure_guard.observe(&call, &result);
                    }
                    // Exact history is derived from the security-sanitized result before the
                    // bounded durable trace projection. This keeps model context compact without
                    // making historical recall lossy.
                    let result_sequence = conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .pending_tool_result_sequence(&call.id);
                    let archive_result = tool_registry.archive_projection(&result);
                    let llm_result = tool_registry.model_projection(&result);
                    let archive_metadata = if tool_registry.archives_result(&call.tool) {
                        archive_tool_result(ToolResultArchiveRequest {
                            storage: exact_history_storage.as_ref(),
                            conversation_id: trace_conversation_id.as_deref(),
                            assistant_message_id: trace_assistant_message_id.as_deref(),
                            sequence: result_sequence,
                            raw_result: &result,
                            archive_result: &archive_result,
                            model_result: &llm_result,
                            model_tool_result_gate: &model_tool_result_gate,
                        })?
                    } else {
                        ConversationHistoryArchiveTraceMetadata::default()
                    };
                    let trace_result = tool_registry.trace_projection(&result);
                    let checkpoint_result = tool_registry.checkpoint_projection(&result);
                    let (model_observation, checkpoint_observation) =
                        match finalize_tool_observations(
                            &model_tool_result_gate,
                            &call.id,
                            !result.ok,
                            &llm_result,
                            &checkpoint_result,
                            &archive_metadata,
                            is_mcp_tool,
                        ) {
                            Ok(observations) => observations,
                            Err(error) => {
                                if settles_entire_provider_tool_batch_on_terminal {
                                    conversation_trace
                                        .lock()
                                        .unwrap_or_else(|poison| poison.into_inner())
                                        .record_tool_result_with_archive(
                                            &call,
                                            &checkpoint_result,
                                            archive_metadata,
                                        );
                                    settle_aborted_grouped_tool_batch(
                                        &error,
                                        Some(TerminalToolCallSettlement {
                                            queued: cancellation_queued_tool_call.clone(),
                                            call: Some(call.clone()),
                                            announced: true,
                                            dispatch_started: true,
                                            outcome: TerminalToolCallOutcome::Synthetic,
                                        }),
                                        &mut tool_batch,
                                        &mut pending_assistant_context,
                                        &mut active_context,
                                        &conversation_trace,
                                        trace_observer.as_ref(),
                                        &mut event_stream,
                                        tool_registry.as_ref(),
                                        &model_tool_result_gate,
                                        trace_assistant_message_id.as_deref(),
                                        &run_id,
                                    )?;
                                }
                                return Err(error);
                            }
                        };
                    let recorded_result_sequence = {
                        let mut recorder = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let sequence = recorder.record_tool_result_with_archive(
                            &call,
                            &checkpoint_result,
                            archive_metadata.clone(),
                        );
                        if let Some(sequence) = sequence {
                            recorder
                                .record_model_message(
                                sequence,
                                0,
                                &LlmMessage::tool_result(
                                    call.id.clone(),
                                    checkpoint_observation.clone(),
                                    !result.ok,
                                ),
                            )
                                .map_err(AgentError::new)?;
                        }
                        sequence
                    };
                    let trace_publish = if tool_result_persistence
                        == crate::tools::AgentToolResultPersistence::PrecommittedTrace
                    {
                        Ok(())
                    } else {
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())
                            .map(|_| ())
                    };
                    if let Err(error) = trace_publish {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_aborted_grouped_tool_batch(
                                &error,
                                Some(TerminalToolCallSettlement {
                                    queued: cancellation_queued_tool_call.clone(),
                                    call: Some(call.clone()),
                                    announced: true,
                                    dispatch_started: true,
                                    outcome: TerminalToolCallOutcome::Settled(Box::new(
                                        SettledTerminalToolCallOutcome {
                                        result: result.clone(),
                                        checkpoint_result: checkpoint_result.clone(),
                                        model_observation: model_observation.clone(),
                                        checkpoint_observation: checkpoint_observation.clone(),
                                        archive_metadata: archive_metadata.clone(),
                                        },
                                    )),
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                None,
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Err(error);
                    }
                    if cancellation_preempts_tool_result(
                        auto_execute_host_action,
                        if authoritative_tool_settlement {
                            crate::tools::AgentToolCancellationSettlement::Authoritative
                        } else {
                            crate::tools::AgentToolCancellationSettlement::Interruptible
                        },
                        cancellation_token.is_cancelled(),
                        &result,
                    ) {
                        if settles_entire_provider_tool_batch_on_terminal {
                            settle_cancelled_grouped_tool_batch(
                                Some(TerminalToolCallSettlement {
                                    queued: cancellation_queued_tool_call.clone(),
                                    call: Some(call.clone()),
                                    announced: true,
                                    dispatch_started: true,
                                    outcome: TerminalToolCallOutcome::Settled(Box::new(
                                        SettledTerminalToolCallOutcome {
                                        result: result.clone(),
                                        checkpoint_result: checkpoint_result.clone(),
                                        model_observation: model_observation.clone(),
                                        checkpoint_observation: checkpoint_observation.clone(),
                                        archive_metadata: archive_metadata.clone(),
                                        },
                                    )),
                                }),
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
                    if !is_mcp_tool {
                        let event_result =
                            redact_tool_result_for_event(&tool_registry.event_projection(&result));
                        event_stream.emit(AgentEvent::ToolResult {
                            run_id: run_id.clone(),
                            result: event_result.clone(),
                        });
                        if let Some(draft) = file_draft_from_tool_result(&event_result) {
                            event_stream.emit(AgentEvent::FileDraftUpdated {
                                run_id: run_id.clone(),
                                draft,
                            });
                        }
                    }

                    active_context.push(
                        ContextItem::tool_result(
                            call.id.clone(),
                            model_observation.clone(),
                            !result.ok,
                            with_trace_origin(
                                ContextMetadata::new(
                                    ContextSource::ToolResult,
                                    ContextScope::Run,
                                    ContextRetention::Retained,
                                )
                                .with_group(tool_exchange_group.clone()),
                                trace_assistant_message_id.as_deref(),
                                recorded_result_sequence,
                            ),
                        )
                        .with_checkpoint_tool_result(
                            call.id.clone(),
                            checkpoint_observation,
                            !result.ok,
                        ),
                    );
                    if let Some(image_message) =
                        llm_image_message_from_tool_result(&result, model_capabilities)
                    {
                        let checkpoint_message = LlmMessage::text(
                            image_message.role(),
                            image_message.content().to_string(),
                        );
                        active_context.push(
                            ContextItem::new(
                                image_message,
                                with_trace_origin(
                                    ContextMetadata::new(
                                        ContextSource::ToolResult,
                                        ContextScope::Run,
                                        ContextRetention::Retained,
                                    )
                                    .with_group(tool_exchange_group.clone()),
                                    trace_assistant_message_id.as_deref(),
                                    recorded_result_sequence,
                                ),
                            )
                            .with_checkpoint_message(checkpoint_message),
                        );
                    }
                    let extension_effects = match runtime_extensions.on_event(
                        RuntimeExtensionEvent::ToolCompleted {
                            result: &trace_result,
                        },
                    ) {
                        Ok(effects) => effects,
                        Err(error) => {
                            if settles_entire_provider_tool_batch_on_terminal
                                && !tool_batch.is_empty()
                            {
                                settle_aborted_grouped_tool_batch(
                                    &error,
                                    None,
                                    &mut tool_batch,
                                    &mut pending_assistant_context,
                                    &mut active_context,
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                    &mut event_stream,
                                    tool_registry.as_ref(),
                                    &model_tool_result_gate,
                                    trace_assistant_message_id.as_deref(),
                                    &run_id,
                                )?;
                            }
                            return Err(error);
                        }
                    };
                    // A tool result must remain adjacent to its assistant tool call. Runtime
                    // extensions may append retained context only after the paired result has
                    // entered the frame, otherwise provider tool-call protocol would be invalid.
                    for effect in extension_effects {
                        match effect {
                            RuntimeEffect::EmitEvent(event) => event_stream.emit(*event),
                            RuntimeEffect::AppendRetainedContext(item) => {
                                active_context.push(*item)
                            }
                        }
                    }
                    if terminate_after_repeat_guard_result {
                        let error = ToolFailureGuard::terminal_error(&call);
                        if settles_entire_provider_tool_batch_on_terminal && !tool_batch.is_empty()
                        {
                            settle_aborted_grouped_tool_batch(
                                &error,
                                None,
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Err(error);
                    }
                    if cancellation_token.is_cancelled() {
                        if settles_entire_provider_tool_batch_on_terminal && !tool_batch.is_empty()
                        {
                            settle_cancelled_grouped_tool_batch(
                                None,
                                &mut tool_batch,
                                &mut pending_assistant_context,
                                &mut active_context,
                                &conversation_trace,
                                trace_observer.as_ref(),
                                &mut event_stream,
                                tool_registry.as_ref(),
                                &model_tool_result_gate,
                                trace_assistant_message_id.as_deref(),
                                &run_id,
                            )?;
                        }
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
                }
            };

            if cancellation_token.is_cancelled() {
                return Ok(cancelled_output(
                    run_id,
                    event_stream,
                    tool_definitions,
                    runtime_extensions.todo_state(),
                    usage,
                    finish_reason,
                ));
            }
            let final_file_transactions = FileTransactionState::load(
                transaction_storage.as_deref(),
                &run_id,
                trace_conversation_id.as_deref(),
                run_context
                    .as_ref()
                    .and_then(|context| context.project_id.as_deref()),
            )?;
            if final_file_transactions.blocks_user_text() {
                return Err(AgentError::new(
                    "文件事务尚未结算，不能结束当前运行或输出最终回复。",
                ));
            }
            file_transaction_guard.complete();
            let content = final_content;
            if !llm_request.stream {
                event_stream.emit(AgentEvent::MessageDelta {
                    run_id: run_id.clone(),
                    stream_id: None,
                    delta: content.clone(),
                });
            }
            event_stream.emit(state_event(&run_id, AgentRunStatus::Completed, None, None));
            event_stream.emit(done_event(
                &run_id,
                true,
                AgentRunStatus::Completed,
                Some(content.clone()),
                usage.clone(),
                finish_reason.clone(),
                Vec::new(),
            ));

            Ok(AgentChatOutput {
                content,
                status: AgentRunStatus::Completed,
                run_id,
                events: event_stream.into_events(),
                tool_definitions,
                todo: runtime_extensions.todo_state(),
                usage,
                finish_reason,
                proposed_actions: Vec::<AgentProposedAction>::new(),
                conversation_turn_trace: None,
            })
        }
        .await;

        finalize_runtime_trace(
            result,
            &conversation_trace,
            &trace_run_id,
            trace_conversation_id.as_deref(),
            trace_assistant_message_id.as_deref(),
        )
    }
}

/// Revalidates every authority-bearing ToolCall identity frozen into a resumed run.
///
/// A conclusively failed `Unregistered` call is retained only as paired audit history; it never
/// becomes checkpoint, approval, or execution authority. Any unresolved unknown call, legacy
/// identity, or registered tool whose implementation identity changed remains fail-closed even
/// when its provider-visible name stayed the same.
fn validate_resumed_tool_provenance(
    recorder: &ConversationTraceRecorder,
    tool_registry: &ToolRegistry,
) -> AgentResult<()> {
    let snapshot = recorder.checkpoint_snapshot();
    for (item_index, item) in snapshot.items.iter().enumerate() {
        let ConversationTurnTraceItem::ToolCall {
            sequence: call_sequence,
            call_id,
            tool,
            provenance,
            approval_status,
            ..
        } = item
        else {
            continue;
        };

        if let AgentToolIdentity::Unregistered { tool_name } = provenance {
            // An unknown model call is allowed to remain in the immutable audit prefix only after
            // it has been conclusively rejected. It carries no execution authority into the
            // resumed batch. Unresolved, mismatched, or non-failure records remain fail-closed.
            let has_matching_failure = tool_name == tool
                && *approval_status == AgentApprovalStatus::NotRequired
                && matches!(
                    snapshot.items.get(item_index.saturating_add(1)),
                    Some(ConversationTurnTraceItem::ToolResult {
                        sequence: result_sequence,
                        call_id: result_call_id,
                        tool: result_tool,
                        status: crate::conversation_trace::ConversationTraceToolResultStatus::Failed,
                        success: false,
                        approval_status: AgentApprovalStatus::NotRequired,
                        ..
                    }) if result_sequence > call_sequence
                        && result_call_id == call_id
                        && result_tool == tool
                );
            if has_matching_failure {
                continue;
            }
            return Err(AgentError::new(
                "运行检查点的工具来源与当前冻结工具注册不一致。",
            ));
        }

        if matches!(
            provenance,
            AgentToolIdentity::LegacyBuiltinCapability { .. }
        ) || tool_registry.identity(tool) != Some(provenance)
        {
            return Err(AgentError::new(
                "运行检查点的工具来源与当前冻结工具注册不一致。",
            ));
        }
    }
    Ok(())
}

type PendingAssistantToolContext = (LlmMessage, LlmMessage, crate::context::ContextGroup);

/// Owns the narrow interval between sealing a private Provider turn and publishing its first
/// durable Host handoff. Staged rows are invisible to hydration, profile-boundary detection,
/// forks and approval checkpoints until this exact binding is promoted.
struct PendingProviderContinuationHandoff {
    vault: Arc<crate::ProviderContinuationVault>,
    continuation_ref: crate::protocol::ProviderContinuationRef,
    conversation_id: String,
    assistant_message_id: String,
    run_id: String,
    request_index: u64,
    assistant_turn_id: String,
    assistant_turn_digest: String,
    provider_protocol: ProviderProtocolKey,
}

impl PendingProviderContinuationHandoff {
    fn binding(&self) -> crate::provider_continuation_store::ProviderContinuationBinding<'_> {
        crate::provider_continuation_store::ProviderContinuationBinding {
            conversation_id: &self.conversation_id,
            assistant_message_id: &self.assistant_message_id,
            run_id: &self.run_id,
            request_index: self.request_index,
            assistant_turn_id: &self.assistant_turn_id,
            assistant_turn_digest: &self.assistant_turn_digest,
            provider_protocol: &self.provider_protocol,
        }
    }
}

fn promote_pending_provider_continuation(
    pending: &mut Option<PendingProviderContinuationHandoff>,
) -> AgentResult<()> {
    let Some(handoff) = pending.as_ref() else {
        return Ok(());
    };
    handoff
        .vault
        .promote_staged(&handoff.continuation_ref, handoff.binding(), now_ms())
        .map_err(provider_continuation_runtime_error)?;
    *pending = None;
    Ok(())
}

fn release_pending_provider_continuation(
    pending: &mut Option<PendingProviderContinuationHandoff>,
) -> AgentResult<()> {
    let Some(handoff) = pending.as_ref() else {
        return Ok(());
    };
    handoff
        .vault
        .release(
            &handoff.continuation_ref,
            crate::provider_continuation_store::ProviderContinuationOwner {
                conversation_id: &handoff.conversation_id,
                assistant_message_id: &handoff.assistant_message_id,
                run_id: &handoff.run_id,
            },
            now_ms(),
        )
        .map_err(provider_continuation_runtime_error)?;
    *pending = None;
    Ok(())
}

/// Some provider protocols persist a tool-bearing Assistant Turn before any Tool can execute.
/// Once that happens, cancellation must close the *whole* grouped turn as well: leaving even one
/// call without a ToolResult would make the encrypted continuation impossible to replay safely.
///
/// `call` carries policy/approval state already computed for the in-flight call. Queued suffix
/// calls deliberately use a fresh `NotRequired` state because cancellation prevents them from
/// reaching either approval or dispatch.
struct TerminalToolCallSettlement {
    queued: QueuedToolCall,
    call: Option<AgentToolCall>,
    announced: bool,
    dispatch_started: bool,
    outcome: TerminalToolCallOutcome,
}

enum TerminalToolCallOutcome {
    Synthetic,
    Authoritative(AgentToolResult),
    Settled(Box<SettledTerminalToolCallOutcome>),
}

struct SettledTerminalToolCallOutcome {
    result: AgentToolResult,
    checkpoint_result: AgentToolResult,
    model_observation: String,
    checkpoint_observation: String,
    archive_metadata: ConversationHistoryArchiveTraceMetadata,
}

#[derive(Clone)]
enum GroupedToolBatchTerminalCause {
    Cancelled,
    Aborted { cause_code: String },
}

fn take_pending_assistant_tool_context(
    tool_batch: &mut ToolCallBatch,
) -> AgentResult<Option<PendingAssistantToolContext>> {
    let checkpoint_assistant_message = tool_batch.checkpoint_assistant_message()?;
    match (
        tool_batch.take_assistant_turn(),
        checkpoint_assistant_message,
        tool_batch.context_group(),
    ) {
        (Some(turn), Some(checkpoint_message), Some(group)) => Ok(Some((
            LlmMessage::from_assistant_turn(turn),
            checkpoint_message,
            group,
        ))),
        (None, None, _) => Ok(None),
        _ => Err(AgentError::new(
            "Tool Call 批次的完整 Assistant Turn 与执行队列不一致。",
        )),
    }
}

fn cancelled_tool_call_result(call: &AgentToolCall, dispatch_started: bool) -> AgentToolResult {
    let dispatch = if dispatch_started {
        json!({
            "dispatchCertainty": "possiblyDispatched",
            "retryable": false,
            "recovery": "inspectAuthoritativeStateBeforeRetry",
        })
    } else {
        json!({
            "dispatchCertainty": "definitelyNotDispatched",
            "executed": false,
            "retryable": false,
            "recovery": "waitForExplicitUserInstruction",
        })
    };
    let mut details = json!({
        "type": "runtimeGuard",
        "code": "runCancelled",
        "status": "cancelled",
        "outcome": "cancelled",
        "cancelled": true,
    });
    if let (Some(details), Some(dispatch)) = (details.as_object_mut(), dispatch.as_object()) {
        details.extend(dispatch.clone());
    }
    failed_tool_call_result(
        call,
        AgentError::structured("agent.run_cancelled", "agent run 已取消。", details),
    )
}

fn aborted_tool_call_result(
    call: &AgentToolCall,
    dispatch_started: bool,
    cause_code: &str,
) -> AgentToolResult {
    let (outcome, dispatch_certainty, recovery) = if dispatch_started {
        (
            "failed",
            "possiblyDispatched",
            "inspectAuthoritativeStateBeforeRetry",
        )
    } else {
        (
            "skipped",
            "definitelyNotDispatched",
            "retryFromSafeContextBoundary",
        )
    };
    let mut details = json!({
        "type": "runtimeGuard",
        "code": "groupedTurnAborted",
        "status": "failed",
        "outcome": outcome,
        "dispatchCertainty": dispatch_certainty,
        "retryable": false,
        "recovery": recovery,
        "causeCode": cause_code,
    });
    if !dispatch_started {
        details["executed"] = json!(false);
    }
    failed_tool_call_result(
        call,
        AgentError::structured(
            "agent.grouped_tool_call_skipped",
            "The grouped Provider turn was aborted before this Tool Call could complete.",
            details,
        ),
    )
}

/// Atomically stages ordered cancellation ToolResults for the current call and every queued
/// suffix call, then publishes one complete trace snapshot. No remaining Tool is proposed,
/// approved, or dispatched. Independent-call profiles retain their historical cancellation
/// behavior.
#[allow(clippy::too_many_arguments)]
fn settle_cancelled_grouped_tool_batch(
    current: Option<TerminalToolCallSettlement>,
    tool_batch: &mut ToolCallBatch,
    pending_assistant_context: &mut Option<PendingAssistantToolContext>,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    event_stream: &mut AgentEventStream,
    tool_registry: &ToolRegistry,
    model_tool_result_gate: &ModelToolResultGate,
    trace_assistant_message_id: Option<&str>,
    run_id: &str,
) -> AgentResult<()> {
    settle_terminal_grouped_tool_batch(
        GroupedToolBatchTerminalCause::Cancelled,
        current,
        tool_batch,
        pending_assistant_context,
        active_context,
        conversation_trace,
        trace_observer,
        event_stream,
        tool_registry,
        model_tool_result_gate,
        trace_assistant_message_id,
        run_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn settle_aborted_grouped_tool_batch(
    cause: &AgentError,
    current: Option<TerminalToolCallSettlement>,
    tool_batch: &mut ToolCallBatch,
    pending_assistant_context: &mut Option<PendingAssistantToolContext>,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    event_stream: &mut AgentEventStream,
    tool_registry: &ToolRegistry,
    model_tool_result_gate: &ModelToolResultGate,
    trace_assistant_message_id: Option<&str>,
    run_id: &str,
) -> AgentResult<()> {
    settle_terminal_grouped_tool_batch(
        GroupedToolBatchTerminalCause::Aborted {
            cause_code: cause.code().unwrap_or("agent.runtime_abort").to_string(),
        },
        current,
        tool_batch,
        pending_assistant_context,
        active_context,
        conversation_trace,
        trace_observer,
        event_stream,
        tool_registry,
        model_tool_result_gate,
        trace_assistant_message_id,
        run_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn settle_terminal_grouped_tool_batch(
    terminal_cause: GroupedToolBatchTerminalCause,
    current: Option<TerminalToolCallSettlement>,
    tool_batch: &mut ToolCallBatch,
    pending_assistant_context: &mut Option<PendingAssistantToolContext>,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    event_stream: &mut AgentEventStream,
    tool_registry: &ToolRegistry,
    model_tool_result_gate: &ModelToolResultGate,
    trace_assistant_message_id: Option<&str>,
    run_id: &str,
) -> AgentResult<()> {
    let mut staged_batch = tool_batch.clone();
    let mut staged_context = active_context.clone();
    let mut staged_pending_assistant_context = pending_assistant_context.clone();
    let mut staged_trace = conversation_trace
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let mut settlements = Vec::with_capacity(staged_batch.len().saturating_add(1));
    if let Some(current) = current {
        settlements.push(current);
    }
    while let Some(queued) = staged_batch.pop_front() {
        settlements.push(TerminalToolCallSettlement {
            queued,
            call: None,
            announced: false,
            dispatch_started: false,
            outcome: TerminalToolCallOutcome::Synthetic,
        });
    }

    let mut staged_events = Vec::with_capacity(settlements.len().saturating_mul(2));
    for settlement in settlements {
        let TerminalToolCallSettlement {
            queued,
            call,
            announced,
            dispatch_started,
            outcome,
        } = settlement;
        let call = call.unwrap_or_else(|| AgentToolCall {
            id: queued.call.id.clone(),
            tool: queued.call.name.clone(),
            args: queued.call.args.clone(),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: extract_reason_from_args(&queued.call.args),
        });
        let model_context_provider_identity = queued.provider_identity()?;
        let tool_identity = tool_registry
            .identity(&call.tool)
            .cloned()
            .unwrap_or_else(|| AgentToolIdentity::Unregistered {
                tool_name: call.tool.clone(),
            });
        let is_mcp_tool = matches!(&tool_identity, AgentToolIdentity::Mcp { .. });
        let trace_call = tool_registry.trace_call_projection(&call);
        let checkpoint_call = tool_registry.checkpoint_call_projection(&call);
        let durable_trace_assistant_message = LlmMessage::assistant(
            queued.assistant_content.clone(),
            vec![queued.checkpoint_call.clone()],
        );
        let call_sequence =
            staged_trace.record_tool_call_with_identity(&trace_call, tool_identity.clone());
        if let Some(sequence) = call_sequence {
            staged_trace
                .record_model_tool_call_message(
                    sequence,
                    0,
                    &durable_trace_assistant_message,
                    model_context_provider_identity,
                )
                .map_err(AgentError::new)?;
            if trace_call.args != call.args || checkpoint_call.args != call.args {
                staged_trace.mark_truncated();
            }
        }

        let tool_exchange_group = queued.context_group();
        if let Some((live_message, checkpoint_message, batch_group)) =
            staged_pending_assistant_context.take()
        {
            if batch_group != tool_exchange_group {
                return Err(AgentError::new(
                    "Tool Call 取消结算的 Assistant Turn 与结果分组不一致。",
                ));
            }
            staged_context.push(
                ContextItem::new(
                    live_message,
                    with_trace_origin(
                        ContextMetadata::new(
                            ContextSource::ModelResponse,
                            ContextScope::Run,
                            ContextRetention::Retained,
                        )
                        .with_group(batch_group),
                        trace_assistant_message_id,
                        call_sequence,
                    ),
                )
                .with_checkpoint_message(checkpoint_message),
            );
        }

        if !announced && !is_mcp_tool {
            staged_events.push(AgentEvent::ToolCall {
                run_id: run_id.to_string(),
                trace_sequence: call_sequence
                    .expect("a settled ToolCall always has a durable trace sequence"),
                call: tool_registry.event_call_projection(&call),
                identity: tool_identity,
            });
        }

        let project_terminal_result = |result: AgentToolResult| {
            let model_result = tool_registry.model_projection(&result);
            let checkpoint_result = tool_registry.checkpoint_projection(&result);
            let archive_metadata = ConversationHistoryArchiveTraceMetadata::default();
            let (model_observation, checkpoint_observation) = finalize_tool_observations(
                model_tool_result_gate,
                &call.id,
                true,
                &model_result,
                &checkpoint_result,
                &archive_metadata,
                is_mcp_tool,
            )?;
            Ok::<_, AgentError>((
                result,
                checkpoint_result,
                model_observation,
                checkpoint_observation,
                archive_metadata,
            ))
        };
        let (
            result,
            checkpoint_result,
            model_observation,
            checkpoint_observation,
            archive_metadata,
        ) = match outcome {
            TerminalToolCallOutcome::Settled(settled) => {
                let SettledTerminalToolCallOutcome {
                    result,
                    checkpoint_result,
                    model_observation,
                    checkpoint_observation,
                    archive_metadata,
                } = *settled;
                (
                    result,
                    checkpoint_result,
                    model_observation,
                    checkpoint_observation,
                    archive_metadata,
                )
            }
            TerminalToolCallOutcome::Authoritative(result) => project_terminal_result(result)?,
            TerminalToolCallOutcome::Synthetic => project_terminal_result(match &terminal_cause {
                GroupedToolBatchTerminalCause::Cancelled => {
                    cancelled_tool_call_result(&call, dispatch_started)
                }
                GroupedToolBatchTerminalCause::Aborted { cause_code } => {
                    aborted_tool_call_result(&call, dispatch_started, cause_code)
                }
            })?,
        };
        let is_error = !result.ok;
        let result_sequence = staged_trace.record_tool_result_with_archive(
            &call,
            &checkpoint_result,
            archive_metadata,
        );
        if let Some(sequence) = result_sequence {
            staged_trace
                .record_model_message(
                    sequence,
                    0,
                    &LlmMessage::tool_result(
                        call.id.clone(),
                        checkpoint_observation.clone(),
                        is_error,
                    ),
                )
                .map_err(AgentError::new)?;
        }
        if !is_mcp_tool {
            staged_events.push(AgentEvent::ToolResult {
                run_id: run_id.to_string(),
                result: redact_tool_result_for_event(&tool_registry.event_projection(&result)),
            });
        }
        staged_context.push(
            ContextItem::tool_result(
                call.id.clone(),
                model_observation,
                is_error,
                with_trace_origin(
                    ContextMetadata::new(
                        ContextSource::ToolResult,
                        ContextScope::Run,
                        ContextRetention::Retained,
                    )
                    .with_group(tool_exchange_group),
                    trace_assistant_message_id,
                    result_sequence,
                ),
            )
            .with_checkpoint_tool_result(call.id, checkpoint_observation, is_error),
        );
    }
    staged_context.validate_complete_tool_protocol()?;

    *tool_batch = staged_batch;
    *active_context = staged_context;
    *pending_assistant_context = staged_pending_assistant_context;
    *conversation_trace
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = staged_trace;
    publish_trace_snapshot(conversation_trace, trace_observer)?;
    for event in staged_events {
        event_stream.emit(event);
    }
    Ok(())
}

fn unavailable_tool_error(tool_set: &EffectiveToolSet, tool_name: &str) -> AgentError {
    match tool_set
        .unavailability(tool_name)
        .unwrap_or(ToolUnavailability::NotRegistered)
    {
        ToolUnavailability::RequiresSkillActivation {
            required_capability,
        } => AgentError::structured(
            "agent.tool_requires_skill_activation",
            format!(
                "Tool `{tool_name}` is unavailable until its matching Skill is activated."
            ),
            json!({
                "type": "tool_policy",
                "code": "toolRequiresSkillActivation",
                "recovery": "activateSkill",
                "requiredCapability": required_capability.as_str(),
            }),
        ),
        ToolUnavailability::RequiresBuiltinCapabilityActivation {
            required_capability,
        } => AgentError::structured(
            "agent.tool_requires_builtin_capability_activation",
            format!(
                "Tool `{tool_name}` is unavailable until its built-in capability is activated for this task."
            ),
            json!({
                "type": "tool_policy",
                "code": "toolRequiresBuiltinCapabilityActivation",
                "recovery": "activateCapability",
                "requiredCapability": required_capability.as_str(),
                "retryable": false,
            }),
        ),
        ToolUnavailability::BlockedByPermissions => AgentError::structured(
            "agent.tool_blocked_by_permissions",
            format!("Tool `{tool_name}` is disabled by the current permission policy."),
            json!({
                "type": "tool_policy",
                "code": "toolBlockedByPermissions",
                "recovery": "changePermissions",
                "bypassAllowed": false,
            }),
        ),
        ToolUnavailability::RuntimeCapabilityUnavailable {
            required_capability,
        } => AgentError::structured(
            "agent.tool_runtime_capability_unavailable",
            format!(
                "Tool `{tool_name}` cannot run because its required application capability is not available in this runtime."
            ),
            json!({
                "type": "tool_policy",
                "code": "toolRuntimeCapabilityUnavailable",
                "recovery": "configureCapability",
                "requiredCapability": required_capability.as_str(),
                "retryable": false,
            }),
        ),
        ToolUnavailability::NotRegistered => AgentError::structured(
            "agent.tool_not_registered",
            format!("Tool `{tool_name}` is not registered in the current application runtime."),
            json!({
                "type": "tool_policy",
                "code": "toolNotRegistered",
                "recovery": "useAvailableTool",
                "retryable": false,
            }),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_steer_inputs(
    run_id: &str,
    trace_assistant_message_id: Option<&str>,
    preceding_assistant_content: Option<&str>,
    inputs: Vec<AgentSteerInput>,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    event_stream: &mut AgentEventStream,
    tool_context: &mut ToolExecutionContext,
    run_context: &mut Option<AgentRunContext>,
) -> AgentResult<Option<AgentContextBaseline>> {
    let attachment_contexts = inputs
        .iter()
        .map(|input| {
            if !input.attachments.is_empty() && input.attachment_library.is_none() {
                return Err(AgentError::structured(
                    "agent.steer_attachment_library_missing",
                    "用户引导附件缺少 Host 构造的附件库快照。",
                    json!({ "guidanceId": input.guidance_id }),
                ));
            }
            build_attachment_context(&input.attachments, input.attachment_library.as_ref())
        })
        .collect::<AgentResult<Vec<_>>>()?;

    let preceding_assistant_content = preceding_assistant_content
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string);
    let mut applied = Vec::with_capacity(inputs.len());
    let preceding_assistant_sequence;
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        preceding_assistant_sequence = match preceding_assistant_content.as_deref() {
            Some(content) => recorder
                .record_narration(content)
                .map_err(AgentError::new)?,
            None => None,
        };
        for input in &inputs {
            let sequence = recorder
                .record_user_guidance(
                    &input.guidance_id,
                    &input.client_message_id,
                    &input.content,
                    &input.attachments,
                    input.created_at,
                )
                .ok_or_else(|| {
                    AgentError::structured(
                        "agent.invalid_steer_input",
                        "用户引导正文不能为空。",
                        json!({ "guidanceId": input.guidance_id }),
                    )
                })?;
            let (attachments, _) = trace_attachments_from_input(&input.attachments);
            applied.push((input.clone(), attachments, sequence));
        }
    }
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for ((input, _, sequence), attachment_context) in
            applied.iter().zip(attachment_contexts.iter())
        {
            let content = input.content.trim();
            let context_content = if attachment_context.text.trim().is_empty() {
                content.to_string()
            } else {
                format!("{content}\n\n{}", attachment_context.text)
            };
            let mut message = LlmMessage::text(LlmMessageRole::User, context_content);
            *message
                .images_mut()
                .expect("user attachment messages support images") =
                attachment_context.images.clone();
            recorder
                .record_model_message(*sequence, 0, &message)
                .map_err(AgentError::new)?;
        }
    }
    let baseline = publish_trace_snapshot(conversation_trace, trace_observer)?;

    if let Some(content) = preceding_assistant_content {
        active_context.push(ContextItem::new(
            LlmMessage::text(LlmMessageRole::Assistant, content),
            with_trace_origin(
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ),
                trace_assistant_message_id,
                preceding_assistant_sequence,
            ),
        ));
    }
    for ((input, attachments, sequence), attachment_context) in
        applied.into_iter().zip(attachment_contexts)
    {
        if let Some(library) = input.attachment_library {
            replace_runtime_attachment_library(run_context, tool_context, library);
        }
        let content = input.content.trim().to_string();
        let context_content = if attachment_context.text.trim().is_empty() {
            content.clone()
        } else {
            format!("{content}\n\n{}", attachment_context.text)
        };
        let mut message = LlmMessage::text(LlmMessageRole::User, context_content);
        *message
            .images_mut()
            .expect("user attachment messages support images") = attachment_context.images;
        active_context.push(ContextItem::new(
            message,
            with_trace_origin(
                ContextMetadata::new(
                    ContextSource::UserGuidance,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ),
                trace_assistant_message_id,
                Some(sequence),
            ),
        ));
        event_stream.emit(AgentEvent::GuidanceApplied {
            run_id: run_id.to_string(),
            guidance_id: input.guidance_id,
            client_message_id: input.client_message_id,
            content,
            attachments,
            created_at: input.created_at,
            sequence,
        });
    }

    Ok(baseline)
}

fn apply_agent_mailbox_delivery(
    delivery: &AgentSamplingBoundaryDelivery,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    assistant_message_id: &str,
) -> AgentResult<()> {
    let mut newly_recorded = Vec::new();
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for message in &delivery.messages {
            if let Some(content) = recorder
                .record_agent_mailbox_delivery(
                    message.trace_sequence,
                    &delivery.receipt_id,
                    &message.message_id,
                    &message.sender_agent_id,
                    &message.sender_task_name,
                    &message.sender_task_path,
                    message.kind,
                    &message.content,
                    message.created_at,
                )
                .map_err(AgentError::new)?
            {
                newly_recorded.push((message.clone(), content));
            }
        }
    }
    if newly_recorded.is_empty() {
        return Ok(());
    }
    publish_trace_snapshot(conversation_trace, trace_observer)?;
    for (message, content) in newly_recorded {
        active_context.push(ContextItem::new(
            LlmMessage::text(LlmMessageRole::User, content),
            with_trace_origin(
                ContextMetadata::new(
                    ContextSource::UserGuidance,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ),
                Some(assistant_message_id),
                Some(message.trace_sequence),
            ),
        ));
    }
    Ok(())
}

fn with_trace_origin(
    metadata: ContextMetadata,
    assistant_message_id: Option<&str>,
    sequence: Option<u64>,
) -> ContextMetadata {
    match assistant_message_id.zip(sequence) {
        Some((assistant_message_id, sequence)) => metadata.with_origin(
            ContextOrigin::conversation_trace_item(assistant_message_id, sequence),
        ),
        None => metadata,
    }
}

fn replace_runtime_attachment_library(
    run_context: &mut Option<AgentRunContext>,
    tool_context: &mut ToolExecutionContext,
    library: crate::protocol::AgentAttachmentLibraryContext,
) {
    let context = run_context.get_or_insert_with(|| AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    context.attachment_library = Some(library.clone());
    tool_context.replace_attachment_library(library);
}

struct ToolResultArchiveRequest<'a> {
    storage: Option<&'a Arc<StorageService>>,
    conversation_id: Option<&'a str>,
    assistant_message_id: Option<&'a str>,
    sequence: Option<u64>,
    raw_result: &'a AgentToolResult,
    archive_result: &'a AgentToolResult,
    model_result: &'a AgentToolResult,
    model_tool_result_gate: &'a ModelToolResultGate,
}

fn archive_tool_result(
    request: ToolResultArchiveRequest<'_>,
) -> AgentResult<ConversationHistoryArchiveTraceMetadata> {
    let truncated_at_source = crate::tools::tool_result_truncated_at_source(request.raw_result);
    let gate_truncates = request.model_tool_result_gate.would_truncate_with_source(
        &request.model_result.call_id,
        !request.model_result.ok,
        request.model_result,
        truncated_at_source,
    );
    let exact_preview_truncated = request.raw_result.exact_archive_file.is_some()
        && request
            .raw_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation);
    let mut metadata = ConversationHistoryArchiveTraceMetadata {
        truncated_at_source,
        model_projection_truncated: projection_differs(
            request.archive_result,
            request.model_result,
        ) || gate_truncates
            || exact_preview_truncated,
        archive_projection_truncated: projection_differs(
            request.raw_result,
            request.archive_result,
        ),
        ..Default::default()
    };
    let (Some(storage), Some(conversation_id), Some(assistant_message_id), Some(sequence)) = (
        request.storage,
        request.conversation_id,
        request.assistant_message_id,
        request.sequence,
    ) else {
        return Ok(metadata);
    };
    if let Some(archive) = storage
        .resolve_authoritative_command_archive_metadata(
            conversation_id,
            assistant_message_id,
            request.raw_result,
        )
        .map_err(AgentError::new)?
    {
        if request
            .raw_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation)
        {
            metadata.model_projection_truncated = true;
        }
        metadata.truncated_at_source = archive.truncated_at_source;
        metadata.archive_projection_truncated = archive.archive_projection_truncated;
        metadata.archive_ref = archive.archive_ref;
        metadata.content_hash = archive.content_hash;
        metadata.archived_bytes = archive.archived_bytes;
        metadata.archived_completely = archive.archived_completely;
        return Ok(metadata);
    }
    let archive = if let Some(exact_file) = request
        .raw_result
        .exact_archive_file
        .as_ref()
        .or(request.archive_result.exact_archive_file.as_ref())
    {
        storage.archive_conversation_tool_result_file(ConversationHistoryArchiveFileInput {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            sequence,
            call_id: request.archive_result.call_id.clone(),
            tool: request.archive_result.tool.clone(),
            content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            content_path: exact_file.path().to_path_buf(),
            truncated_at_source: metadata.truncated_at_source,
            model_projection_truncated: metadata.model_projection_truncated,
            archive_projection_truncated: metadata.archive_projection_truncated,
            created_at: now_ms(),
        })
    } else {
        let content = match serde_json::to_string(request.archive_result) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("failed to serialize exact history tool result: {error}");
                return Ok(metadata);
            }
        };
        storage.archive_conversation_tool_result(ConversationHistoryArchiveInput {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            sequence,
            call_id: request.archive_result.call_id.clone(),
            tool: request.archive_result.tool.clone(),
            content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            content,
            truncated_at_source: metadata.truncated_at_source,
            model_projection_truncated: metadata.model_projection_truncated,
            archive_projection_truncated: metadata.archive_projection_truncated,
            created_at: now_ms(),
        })
    };
    match archive {
        Ok(archive) => {
            metadata.archive_ref = Some(archive.archive_ref);
            metadata.content_hash = Some(archive.content_hash);
            metadata.archived_bytes = Some(archive.total_bytes);
            metadata.archived_completely = Some(archive.archived_completely);
        }
        Err(error) => {
            // Tool settlement remains authoritative even if the auxiliary archive cannot commit.
            // The missing archive identity makes the loss explicit instead of pretending exact
            // recovery is possible.
            eprintln!("failed to store exact history tool result: {error}");
        }
    }
    Ok(metadata)
}

pub(crate) fn finalize_model_tool_observation(
    gate: &ModelToolResultGate,
    call_id: &str,
    is_error: bool,
    model_result: &AgentToolResult,
    archive: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<String> {
    let recovery = model_tool_result_recovery(archive)?;
    let requires_exact_recovery = archive.model_projection_truncated
        && model_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation);
    let output = if requires_exact_recovery {
        gate.project_with_required_recovery(call_id, is_error, model_result, recovery.as_ref())
    } else {
        gate.project(call_id, is_error, model_result, recovery.as_ref())
    };
    if output.truncated && !model_observation_has_recovery(&output.content) {
        return Err(AgentError::new(format!(
            "工具 `{}` 的模型结果超过 10K token，但没有可用的分页游标或 Exact History 恢复位置。",
            model_result.tool
        )));
    }
    Ok(output.content)
}

/// Applies one semantic Model projection to both the live request and its persisted replay.
///
/// `checkpoint_result` remains the canonical recovery/audit value recorded by the checkpoint and
/// Trace lanes. It must not be rendered into model-context history: otherwise a restart or later
/// turn would observe a larger, different Tool result than the live model saw. MCP is the explicit
/// exception: external result bodies are live-only by policy, so its durable replay must use the
/// persistence-safe checkpoint projection instead.
fn finalize_tool_observations(
    gate: &ModelToolResultGate,
    call_id: &str,
    is_error: bool,
    model_result: &AgentToolResult,
    checkpoint_result: &AgentToolResult,
    archive: &ConversationHistoryArchiveTraceMetadata,
    durable_replay_uses_checkpoint_projection: bool,
) -> AgentResult<(String, String)> {
    let model_observation =
        finalize_model_tool_observation(gate, call_id, is_error, model_result, archive)?;
    let checkpoint_observation = if durable_replay_uses_checkpoint_projection {
        finalize_model_tool_observation(gate, call_id, is_error, checkpoint_result, archive)?
    } else {
        model_observation.clone()
    };
    Ok((model_observation, checkpoint_observation))
}

fn load_continuation_archive_metadata(
    input: &AgentChatInput,
    storage: Option<&StorageService>,
) -> AgentResult<ConversationHistoryArchiveTraceMetadata> {
    let (
        Some(storage),
        Some(checkpoint),
        Some(continuation),
        Some(conversation_id),
        Some(assistant_message_id),
    ) = (
        storage,
        input.resume_checkpoint.as_ref(),
        input.tool_continuation.as_ref(),
        input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.as_deref()),
        input.assistant_message_id.as_deref(),
    )
    else {
        return Ok(ConversationHistoryArchiveTraceMetadata::default());
    };
    // A Host-owned command Session uses a separate immutable archive sequence so it can commit
    // the complete process spool before the ordinary ToolResult (and before an approval
    // continuation) becomes visible. Resolve that opaque, conversation-scoped ref first instead
    // of assuming the archive sequence must equal the ToolResult trace sequence.
    if let Some(archive) = storage
        .resolve_authoritative_command_archive_metadata(
            conversation_id,
            assistant_message_id,
            &continuation.result,
        )
        .map_err(AgentError::new)?
    {
        return Ok(archive);
    }
    let sequence = continuation_result_sequence(checkpoint, &continuation.call.id);
    let Some(archive) = storage
        .find_conversation_history_archive_for_trace_item(
            conversation_id,
            assistant_message_id,
            sequence,
        )
        .map_err(AgentError::new)?
    else {
        return Ok(ConversationHistoryArchiveTraceMetadata::default());
    };
    if archive.call_id != continuation.call.id
        || archive.call_id != continuation.result.call_id
        || archive.tool != continuation.call.tool
        || archive.tool != continuation.result.tool
        || archive.assistant_message_id != assistant_message_id
        || archive.sequence != sequence
    {
        return Err(AgentError::new(
            "审批续跑的 Exact History Archive 与冻结的工具结果身份不匹配。",
        ));
    }
    Ok(ConversationHistoryArchiveTraceMetadata {
        archive_ref: Some(archive.archive_ref),
        content_hash: Some(archive.content_hash),
        archived_bytes: Some(archive.total_bytes),
        archived_completely: Some(archive.archived_completely),
        truncated_at_source: archive.truncated_at_source,
        model_projection_truncated: archive.model_projection_truncated,
        history_projection_truncated: false,
        archive_projection_truncated: archive.archive_projection_truncated,
    })
}

fn model_tool_result_recovery(
    archive: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<Option<ModelToolResultRecovery>> {
    let mut recovery = ModelToolResultRecovery {
        truncated_at_source: Some(archive.truncated_at_source),
        ..Default::default()
    };
    if archive.archived_completely == Some(true) {
        let archive_ref = archive.archive_ref.as_deref().ok_or_else(|| {
            AgentError::new("完整的工具历史归档缺少 archive ref，无法生成模型续读位置。")
        })?;
        let open =
            crate::storage::conversation_history_open::encode_archive_history_open(archive_ref, 0)
                .map_err(AgentError::new)?;
        recovery.history_open = Some(Value::String(open.clone()));
        recovery.continue_with = Some(json!({
            "tool": "conversation_history",
            "args": {
                "open": open
            }
        }));
    }
    Ok(Some(recovery))
}

fn model_observation_has_recovery(content: &str) -> bool {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(content) else {
        return false;
    };
    for field in [
        "continueWith",
        "historyOpen",
        "cursor",
        "nextCursor",
        "nextStartByte",
        "nextStartLine",
        "nextAfterPath",
    ] {
        if object.get(field).is_some_and(|value| !value.is_null()) {
            return true;
        }
    }
    object
        .get("navigation")
        .and_then(Value::as_object)
        .is_some_and(|navigation| navigation.values().any(|value| !value.is_null()))
}

fn projection_differs(left: &AgentToolResult, right: &AgentToolResult) -> bool {
    match (serde_json::to_vec(left), serde_json::to_vec(right)) {
        (Ok(left), Ok(right)) => left != right,
        _ => true,
    }
}

fn auto_executes_builtin_prepared_action(
    permission: crate::AgentBuiltinExecutionPermission,
    call_requires_approval: bool,
    identity: &AgentToolIdentity,
) -> bool {
    permission == crate::AgentBuiltinExecutionPermission::AutoApprove
        && call_requires_approval
        && (matches!(identity, AgentToolIdentity::BuiltinCapability { .. })
            || matches!(
                identity,
                AgentToolIdentity::RuntimeExtension {
                    extension_id,
                    tool_name,
                } if extension_id
                    == crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
                    && tool_name == crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME
            ))
}

#[cfg(test)]
mod tests;
