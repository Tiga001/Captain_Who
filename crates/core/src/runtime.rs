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
mod tool_flow;
mod tool_input_stream;
mod trace;

pub use api::*;
use command_dispatch::*;
use events::*;
use preparation::*;
use trace::*;

use crate::cancellation::AgentCancellationToken;
use crate::command::{
    evaluate_command_policy_with_context, CommandAuthorizationSource, CommandPolicyDecision,
};
use crate::context::{
    AgentContextBaseline, AgentConversationContextState, ContextAssembler, ContextAssemblyInput,
    ContextAttachments, ContextBudgetReport, ContextCapacityDetector, ContextCompactionPlan,
    ContextCompactionPlanner, ContextCompactionQuery, ContextFrame, ContextItem, ContextMetadata,
    ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{canonical_tool_result_for_context, ConversationTraceRecorder};
use crate::file_write::{file_write_approval_route, FileWriteApprovalRoute};
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, LlmChatRequest, LlmMessage,
    LlmMessageRole, LlmStreamEvent,
};
use crate::model_request_observation::{
    ModelRequestEstimate, ModelRequestObservation, ModelRequestObservationBuilder,
    ModelRequestPurpose,
};
use crate::prompts::build_system_prompt;
use crate::protocol::{
    AgentApprovalStatus, AgentChatInput, AgentChatMessage, AgentChatOutput, AgentCommandPermission,
    AgentCommandSafetyPolicy, AgentContextCompactionEventOutcome, AgentContextWindowPhase,
    AgentContextWindowSnapshot, AgentError, AgentEvent, AgentExtensionSnapshot, AgentPermissions,
    AgentPromptPreferences, AgentProposedAction, AgentReadPermission, AgentResult, AgentRunContext,
    AgentRunStatus, AgentSkillActivation, AgentSkillScriptPreflightStatus, AgentToolApprovalMode,
    AgentToolCall, AgentToolDefinition, AgentToolResult, AgentWritePermission,
};
use crate::revision::content_revision;
use crate::storage::service::StorageService;
use crate::tools::{ToolExecutionContext, ToolRegistry};
use crate::usage::merge_total_usage;
use crate::{
    ConversationTraceSnapshot, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
};
use attachments::{build_attachment_context, AttachmentContext};
use checkpoint::{
    create_run_checkpoint, restore_run_checkpoint, RestoredRunCheckpoint, RunCheckpointState,
    ToolCallBatch,
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
use tool_flow::{
    approve_proposed_action, build_tool_observation_message, cancelled_output, done_event,
    enforce_skill_activation_barrier, execute_host_action_on_blocking_thread,
    execute_tool_on_blocking_thread, extract_reason_from_args, failed_tool_call_result,
    file_draft_from_tool_result, generate_run_id, llm_image_message_from_tool_result,
    redact_tool_result_for_event, sanitize_max_tokens, sanitize_temperature, state_event,
    tool_calls_from_response,
};
use tool_input_stream::ToolInputStreamObservers;

const DEFAULT_MAX_TOKENS: u32 = 30_000;
const MAX_MAX_TOKENS: u32 = 128_000;
const DEFAULT_TEMPERATURE: f32 = 0.6;
const MAX_TOOL_ITERATIONS: usize = 10_000;
const MAX_CONTEXT_COMPACTION_ATTEMPTS_PER_REQUEST: usize = 3;

struct LlmRequestTemplate {
    api_url: String,
    api_token: String,
    model: String,
    api_style: crate::protocol::AgentApiStyle,
    context_window_tokens: Option<u32>,
    max_tokens: u32,
    temperature: f32,
    stream: bool,
    tools: Vec<AgentToolDefinition>,
}

impl LlmRequestTemplate {
    fn request(&self, context: ContextFrame) -> LlmChatRequest {
        LlmChatRequest {
            api_url: self.api_url.clone(),
            api_token: self.api_token.clone(),
            model: self.model.clone(),
            api_style: self.api_style,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
            stream: self.stream,
            messages: context.into_messages(),
            tools: self.tools.clone(),
        }
    }
}

struct PreparedLlmRequest {
    template: LlmRequestTemplate,
    context: ContextFrame,
    next_model_request_index: usize,
    tool_batch: ToolCallBatch,
    conversation_trace: ConversationTraceRecorder,
    visible_trace_item_count: usize,
}

struct PreparedRuntimeCapabilities {
    runtime_extensions: RuntimeExtensions,
    tool_registry: Arc<ToolRegistry>,
    tool_definitions: Vec<AgentToolDefinition>,
    command_auto_approve: bool,
    command_permissions: AgentPermissions,
    command_workspace_root: Option<PathBuf>,
    patch_auto_approve: bool,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

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
            context_compaction_services,
            skill_resources,
            skill_activation_resolver,
            office_engine,
            command_runtime_profile_resolver,
        } = host_services.unwrap_or_default();
        let run_id = run_id.unwrap_or_else(generate_run_id);
        let context = input.context.clone();
        let model_capabilities = input.model_capabilities;
        let trace_conversation_id = context
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
        let trace_run_id = run_id.clone();
        let setup_conversation_trace = conversation_trace_from_input_checkpoint(&input);
        let shared_context_baseline =
            publish_trace_recorder_snapshot(&setup_conversation_trace, trace_observer.as_ref())?;
        let restored_checkpoint =
            restore_input_checkpoint(&mut input, &run_id).map_err(|error| {
                attach_failed_runtime_trace(
                    error,
                    &setup_conversation_trace,
                    &trace_run_id,
                    trace_conversation_id.as_deref(),
                    trace_assistant_message_id.as_deref(),
                )
            })?;
        let extension_snapshots = restored_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.extension_snapshots.as_slice())
            .unwrap_or_default();
        let PreparedRuntimeCapabilities {
            mut runtime_extensions,
            tool_registry,
            tool_definitions,
            command_auto_approve,
            command_permissions,
            command_workspace_root,
            patch_auto_approve,
        } = prepare_runtime_capabilities_with_skills(
            &input,
            &run_id,
            extension_snapshots,
            host_executor.is_some(),
            office_engine,
            skill_activation_resolver,
            skill_resources.clone(),
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
        let mut event_stream = AgentEventStream::new(emitter);
        event_stream.emit(AgentEvent::Started {
            run_id: run_id.clone(),
            tool_definitions: tool_definitions.clone(),
        });
        let transaction_storage = storage.clone();
        let context_window_configured = input.context_window_tokens.is_some();
        let mut file_transaction_guard = FileTransactionRunGuard::new(
            transaction_storage.clone(),
            run_id.clone(),
            cancellation_token.clone(),
        );
        let PreparedLlmRequest {
            template: llm_request,
            context: mut active_context,
            mut next_model_request_index,
            mut tool_batch,
            conversation_trace,
            mut visible_trace_item_count,
        } = build_llm_request(
            input,
            &tool_definitions,
            restored_checkpoint,
            shared_context_baseline,
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
        let conversation_trace = Arc::new(Mutex::new(conversation_trace));
        let mut pending_trace_baseline =
            publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
        let capacity_detector = ContextCapacityDetector::for_model(
            &llm_request.model,
            llm_request.api_style,
            &llm_request.tools,
        );
        let tool_output_budget = capacity_detector
            .tool_output_text_budget(llm_request.context_window_tokens, llm_request.max_tokens);
        let tool_context = ToolExecutionContext::from_run_context(context.as_ref())
            .with_cancellation(cancellation_token.clone())
            .with_model_capabilities(model_capabilities)
            .with_runtime_services(run_id.clone(), storage)
            .with_skill_resources(skill_resources)
            .with_command_runtime_profile_resolver(command_runtime_profile_resolver)
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
        let context_capacity_detector = context_window_configured.then_some(capacity_detector);
        let context_compaction_planner = context_window_configured
            .then(|| ContextCompactionPlanner::for_tools(&llm_request.tools));
        let context_compaction_executor = context_compaction_services
            .map(ContextCompactionExecutor::new)
            .filter(|_| context_window_configured);
        if let Some(detector) = &context_capacity_detector {
            detector.prepare_frame(&mut active_context);
        }
        let result: AgentResult<AgentChatOutput> = async {
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

            let final_content = 'agent_loop: loop {
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
                if tool_batch.take_suppressed_narration() {
                    active_context.push(suppressed_narration_context_item());
                }
                if tool_batch.is_empty() && next_model_request_index > self.max_tool_iterations {
                    let message = "工具调用次数超过限制，已停止继续执行。".to_string();
                    event_stream.emit(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        message: message.clone(),
                        recoverable: false,
                        code: None,
                        details: None,
                    });
                    return Err(AgentError::new(message));
                }
                if tool_batch.is_empty() {
                    active_context.validate_complete_tool_protocol()?;
                    let model_request_index = next_model_request_index;
                    next_model_request_index = next_model_request_index.saturating_add(1);
                    let file_transactions =
                        FileTransactionState::load(transaction_storage.as_deref(), &run_id)?;
                    let user_text_blocked = file_transactions.blocks_user_text();
                    let mut compaction_attempts = 0_usize;
                    let (request_context, request_estimate) = loop {
                        let mut request_context = active_context.clone();
                        let mut request_estimate = None;
                        runtime_extensions.contribute_request_context(
                            &ModelRequestContext::agent_work(),
                            &mut request_context,
                        )?;
                        if let Some(context) = file_transactions.request_context() {
                            request_context.push(ContextItem::text(
                                LlmMessageRole::System,
                                context,
                                ContextSource::FileTransaction,
                                ContextScope::Run,
                                ContextRetention::RequestOnly,
                            ));
                        }
                        emit_context_manifest_if_enabled(
                            &run_id,
                            model_request_index + 1,
                            &request_context,
                            &llm_request.tools,
                        );
                        let model_input_capacity = if let Some(detector) = &context_capacity_detector {
                            let report = detector.inspect(
                                &mut request_context,
                                llm_request.context_window_tokens,
                                llm_request.max_tokens,
                            );
                            let compaction_query = report.compaction_query();
                            let compaction_plan = context_compaction_planner
                                .as_ref()
                                .expect(
                                    "configured capacity detector must have a compaction planner",
                                )
                                .plan(
                                    &compaction_query,
                                    &request_context.planning_items()?,
                                    model_request_index == 0,
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
                                            visible_trace_item_count,
                                            &cancellation_token,
                                        )
                                        .await?;
                                    if let Some(attempt) = attempt {
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionStarted {
                                                run_id: run_id.clone(),
                                                operation_id: operation_id.clone(),
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
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionFinished {
                                                run_id: run_id.clone(),
                                                operation_id,
                                                outcome,
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
                                                merge_total_usage(&mut usage, compaction_usage);
                                                active_context = (*baseline)
                                                    .replace_persistent_context(active_context);
                                                detector.prepare_frame(&mut active_context);
                                                // Compaction changes the durable baseline identity.
                                                // Replace any pending pre-compaction trace baseline
                                                // before the next successful model response can
                                                // promote it and resurrect the old full history.
                                                pending_trace_baseline = publish_trace_snapshot(
                                                    &conversation_trace,
                                                    trace_observer.as_ref(),
                                                )?;
                                                compaction_attempts =
                                                    compaction_attempts.saturating_add(1);
                                                continue;
                                            }
                                            Err(error) if error.is_cancelled() => {
                                                merge_total_usage(
                                                    &mut usage,
                                                    error.usage().cloned(),
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
                                                merge_total_usage(
                                                    &mut usage,
                                                    error.usage().cloned(),
                                                );
                                                return Err(error.with_usage(usage));
                                            }
                                        }
                                    }
                                }
                            }
                            request_estimate =
                                Some(ModelRequestEstimate::from_budget_report(&report));
                            let remaining_tokens = report
                                .remaining_input_tokens
                                .map(|remaining| u64::try_from(remaining.max(0)).unwrap_or(0));
                            detector.ensure_sendable(report)?;
                            remaining_tokens.map(|remaining_tokens| ModelInputCapacity {
                                remaining_tokens,
                                text_budget: detector.text_budget(remaining_tokens.max(1)),
                            })
                        } else {
                            None
                        };
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
                    );
                    let request = llm_request.request(request_context);
                    let mut committed_message_stream_id = None;
                    let mut committed_tool_input_preview = None;
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
                                        tool_call_id,
                                        tool,
                                        input_delta,
                                        received_bytes,
                                    } => {
                                        let observation = tool_input_stream.on_delta(
                                            tool_registry.as_ref(),
                                            &tool_context,
                                            &stream_id,
                                            tool_call_index,
                                            tool_call_id.as_deref(),
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
                                                    tool_call_id: tool_call_id.clone(),
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
                                        reason,
                                    } => {
                                        event_stream.emit(AgentEvent::LlmRetry {
                                            run_id: delta_run_id.clone(),
                                            stream_id: stream_id.clone(),
                                            attempt,
                                            max_attempts,
                                            reason,
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
                                        committed_tool_input_preview =
                                            Some((stream_id.clone(), tool_input_stream.attempt()));
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
                                merge_total_usage(&mut usage, error.usage().cloned());
                                return Ok(cancelled_output(
                                    run_id,
                                    event_stream,
                                    tool_definitions,
                                    runtime_extensions.todo_state(),
                                    usage,
                                    finish_reason,
                                ));
                            }
                            merge_total_usage(&mut usage, error.usage().cloned());
                            return Err(error.with_usage(usage));
                        }
                    };
                    merge_total_usage(&mut usage, llm_response.usage);
                    finish_reason = llm_response.finish_reason;

                    // Every persisted trace item present in this successful request has now been
                    // observed by the main model and may join the compactable durable baseline.
                    if let Some(baseline) = pending_trace_baseline.take() {
                        active_context = baseline.promote_committed_trace(active_context);
                        visible_trace_item_count = conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .committed_item_count();
                        if let Some(detector) = &context_capacity_detector {
                            detector.prepare_frame(&mut active_context);
                        }
                    }

                    let tool_requests = tool_calls_from_response(
                        llm_response.tool_calls,
                        &llm_response.content,
                        &run_id,
                        model_request_index,
                    );
                    let (tool_requests, deferred_for_skill_activation) =
                        enforce_skill_activation_barrier(tool_requests);
                    let retained_assistant_content = if user_text_blocked {
                        ""
                    } else {
                        llm_response.content.as_str()
                    };
                    let suppressed_narration =
                        user_text_blocked && !llm_response.content.trim().is_empty();
                    let deferred_activation_guard =
                        (deferred_for_skill_activation > 0).then(|| {
                            format!(
                                "The runtime deferred {deferred_for_skill_activation} tool call(s) that were planned in the same response as skills_activate. Re-evaluate those actions after the Skill activation results and complete instructions are available."
                            )
                        });
                    if let Some(detector) = &context_capacity_detector {
                        let mut retained_tokens = detector
                            .estimate_assistant_tool_batch_tokens(
                                retained_assistant_content,
                                &tool_requests,
                            );
                        if let Some(guard) = &deferred_activation_guard {
                            retained_tokens = retained_tokens.saturating_add(
                                detector.estimate_message_tokens(&LlmMessage::text(
                                    LlmMessageRole::System,
                                    guard,
                                )),
                            );
                        }
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
                    clear_deferred_tool_input_preview(
                        &event_stream,
                        &run_id,
                        deferred_for_skill_activation,
                        &mut committed_tool_input_preview,
                    );
                    if !tool_requests.is_empty()
                        && !user_text_blocked
                        && !llm_response.content.trim().is_empty()
                    {
                        conversation_trace
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .record_narration(&llm_response.content);
                        pending_trace_baseline =
                            publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    }
                    if let Some(stream_id) = committed_message_stream_id.take() {
                        event_stream.emit(AgentEvent::MessageStreamCommitted {
                            run_id: run_id.clone(),
                            stream_id,
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
                        break 'agent_loop llm_response.content;
                    }
                    response_fence_corrections = 0;

                    if model_request_index >= self.max_tool_iterations {
                        let message = "工具调用次数超过限制，已停止继续执行。".to_string();
                        event_stream.emit(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: message.clone(),
                            recoverable: false,
                            code: None,
                            details: None,
                        });
                        return Err(AgentError::new(message));
                    }

                    if let Some(guard) = deferred_activation_guard {
                        active_context.push(ContextItem::text(
                            LlmMessageRole::System,
                            guard,
                            ContextSource::RuntimeGuard,
                            ContextScope::Run,
                            ContextRetention::Retained,
                        ));
                    }
                    tool_batch = ToolCallBatch::from_model_response(
                        &run_id,
                        model_request_index,
                        if user_text_blocked {
                            String::new()
                        } else {
                            llm_response.content.clone()
                        },
                        tool_requests,
                        suppressed_narration,
                    );
                }

                while let Some(queued_tool_call) = tool_batch.pop_front() {
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
                    let tool_exchange_group = queued_tool_call.context_group();
                    active_context.push(ContextItem::assistant(
                        queued_tool_call.assistant_content,
                        vec![queued_tool_call.call.clone()],
                        ContextMetadata::new(
                            ContextSource::ModelResponse,
                            ContextScope::Run,
                            ContextRetention::Retained,
                        )
                        .with_group(tool_exchange_group.clone()),
                    ));
                    let tool_request = queued_tool_call.call;
                    let reason = extract_reason_from_args(&tool_request.args);
                    // The effective definitions are both the model contract and the execution
                    // allowlist. The registry may retain tools hidden by the current permission
                    // mode; a hallucinated or text-fallback call must not resurrect one.
                    let tool_is_exposed = tool_definitions
                        .iter()
                        .any(|definition| definition.name == tool_request.name);
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
                    let is_policy_process_tool =
                        call.tool == "run_command" || call.tool == "skills_run_script";
                    let mut prepared_policy_action = None;
                    let mut policy_preflight_failure = None;
                    let mut auto_execute_policy_action = false;
                    let mut requires_approval = definition_requires_approval;

                    if !tool_is_exposed {
                        policy_preflight_failure = Some(failed_tool_call_result(
                            &call,
                            AgentError::structured(
                                "agent.tool_not_available",
                                format!(
                                    "Tool `{}` is not available under the current runtime capabilities.",
                                    call.tool
                                ),
                                json!({
                                    "type": "tool_policy",
                                    "code": "toolNotAvailable",
                                    "recovery": "changePermissionsOrCapabilities",
                                }),
                            ),
                        ));
                    }

                    if is_policy_process_tool && tool_is_exposed {
                        match tool_registry.proposed_action(&tool_context, &call) {
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
                    let auto_execute_host_action =
                        auto_execute_policy_action || auto_execute_patch;
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
                    let trace_call = tool_registry.trace_call_projection(&call);
                    conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_tool_call(&trace_call);
                    pending_trace_baseline =
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    let event_call = tool_registry.event_call_projection(&call);
                    event_stream.emit(AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        call: event_call,
                    });
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

                    if requires_approval {
                        let action_result = if is_policy_process_tool {
                            prepared_policy_action.take().ok_or_else(|| {
                                AgentError::new(format!(
                                    "{} lost its validated action snapshot before approval.",
                                    call.tool
                                ))
                            })
                        } else {
                            tool_registry.proposed_action(&tool_context, &call)
                        };
                        let action = match action_result {
                            Ok(action) => {
                                conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .enrich_tool_call(&action);
                                pending_trace_baseline = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                )?;
                                action
                            }
                            Err(error) => {
                                let result = failed_tool_call_result(&call, error);
                                let llm_result = canonical_tool_result_for_context(&result);
                                conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .record_tool_result(&call, &llm_result);
                                pending_trace_baseline = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                )?;
                                event_stream.emit(AgentEvent::ToolResult {
                                    run_id: run_id.clone(),
                                    result: redact_tool_result_for_event(&result),
                                });
                                active_context.push(ContextItem::tool_result(
                                    call.id.clone(),
                                    build_tool_observation_message(&llm_result),
                                    true,
                                    ContextMetadata::new(
                                        ContextSource::ToolResult,
                                        ContextScope::Run,
                                        ContextRetention::Retained,
                                    )
                                    .with_group(tool_exchange_group.clone()),
                                ));
                                continue;
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
                        if let AgentProposedAction::Diff { diff } = &action {
                            event_stream.emit(AgentEvent::Diff {
                                run_id: run_id.clone(),
                                diff: diff.clone(),
                            });
                        }
                        let checkpoint = {
                            let trace = conversation_trace
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            create_run_checkpoint(
                                &run_id,
                                RunCheckpointState {
                                    context: &active_context,
                                    next_model_request_index,
                                    tool_batch: &tool_batch,
                                    extension_snapshots: runtime_extensions.snapshots()?,
                                    model_visible_trace_item_count: visible_trace_item_count,
                                    pending_tool_call_id: &call.id,
                                    conversation_trace: &trace,
                                },
                            )?
                        };
                        event_stream.emit(AgentEvent::ApprovalRequired {
                            run_id: run_id.clone(),
                            action: action.clone(),
                            checkpoint,
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
                            tool_registry.proposed_action(&tool_context, &call)
                        };
                        match action_result {
                            Ok(action) => {
                                let action = approve_proposed_action(action);
                                conversation_trace
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .enrich_tool_call(&action);
                                pending_trace_baseline = publish_trace_snapshot(
                                    &conversation_trace,
                                    trace_observer.as_ref(),
                                )?;
                                if let AgentProposedAction::Diff { diff } = &action {
                                    event_stream.emit(AgentEvent::Diff {
                                        run_id: run_id.clone(),
                                        diff: diff.clone(),
                                    });
                                }
                                execute_host_action_on_blocking_thread(
                                    host_executor
                                        .as_ref()
                                        .expect("automatic host action requires host executor")
                                        .clone(),
                                    action,
                                    cancellation_token.clone(),
                                )
                                .await
                            }
                            Err(error) => Ok(failed_tool_call_result(&call, error)),
                        }
                    } else {
                        execute_tool_on_blocking_thread(
                            tool_registry.clone(),
                            tool_context.clone(),
                            call.clone(),
                            cancellation_token.clone(),
                        )
                        .await
                    };
                    let result = match result_result {
                        Ok(result) => result,
                        Err(error) if error.is_cancelled() => {
                            return Ok(cancelled_output(
                                run_id,
                                event_stream,
                                tool_definitions,
                                runtime_extensions.todo_state(),
                                usage,
                                finish_reason,
                            ));
                        }
                        Err(error) => return Err(error),
                    };
                    let llm_result = canonical_tool_result_for_context(&result);
                    let trace_result = tool_registry.trace_projection(&result);
                    let checkpoint_result = tool_registry.checkpoint_projection(&result);
                    conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_tool_result(&call, &trace_result);
                    pending_trace_baseline =
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    if (!auto_execute_host_action && cancellation_token.is_cancelled())
                        || result.error.as_deref() == Some("agent run 已取消。")
                    {
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
                            runtime_extensions.todo_state(),
                            usage,
                            finish_reason,
                        ));
                    }
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

                    active_context.push(
                        ContextItem::tool_result(
                            call.id.clone(),
                            build_tool_observation_message(&llm_result),
                            !result.ok,
                            ContextMetadata::new(
                                ContextSource::ToolResult,
                                ContextScope::Run,
                                ContextRetention::Retained,
                            )
                            .with_group(tool_exchange_group.clone()),
                        )
                        .with_checkpoint_tool_result(
                            call.id.clone(),
                            build_tool_observation_message(&checkpoint_result),
                            !result.ok,
                        ),
                    );
                    if let Some(image_message) = llm_image_message_from_tool_result(&result) {
                        active_context.push(ContextItem::new(
                            image_message,
                            ContextMetadata::new(
                                ContextSource::ToolResult,
                                ContextScope::Run,
                                ContextRetention::Retained,
                            )
                            .with_group(tool_exchange_group.clone()),
                        ));
                    }
                    let extension_effects = runtime_extensions
                        .on_event(RuntimeExtensionEvent::ToolCompleted { result: &result })?;
                    // A tool result must remain adjacent to its assistant tool call. Runtime
                    // extensions may append retained context only after the paired result has
                    // entered the frame, otherwise provider tool-call protocol would be invalid.
                    for effect in extension_effects {
                        match effect {
                            RuntimeEffect::EmitEvent(event) => event_stream.emit(event),
                            RuntimeEffect::AppendRetainedContext(item) => {
                                active_context.push(item)
                            }
                        }
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
            let final_file_transactions =
                FileTransactionState::load(transaction_storage.as_deref(), &run_id)?;
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

fn clear_deferred_tool_input_preview(
    event_stream: &AgentEventStream,
    run_id: &str,
    deferred_call_count: usize,
    committed_preview: &mut Option<(String, usize)>,
) {
    if deferred_call_count == 0 {
        return;
    }
    let Some((stream_id, attempt)) = committed_preview.take() else {
        return;
    };
    // The activation barrier discards every non-activation call from this model response. Any
    // streamed write preview belongs to one of those discarded calls and must not survive into
    // the replanning request.
    event_stream.emit_transient(AgentEvent::FileWritePreviewCleared {
        run_id: run_id.to_string(),
        stream_id,
        attempt,
    });
}

#[cfg(test)]
mod tests;
