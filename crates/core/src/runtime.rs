mod attachments;
mod checkpoint;
mod context_compaction;
mod context_compaction_model;
mod extensions;
mod file_transactions;
mod tool_flow;
mod tool_input_stream;

use crate::cancellation::AgentCancellationToken;
use crate::context::{
    AgentContextBaseline, AgentConversationContextState, ContextAssembler, ContextAssemblyInput,
    ContextAttachments, ContextBudgetReport, ContextCapacityDetector, ContextCompactionPlan,
    ContextCompactionPlanner, ContextCompactionQuery, ContextFrame, ContextItem, ContextMetadata,
    ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{canonical_tool_result_for_context, ConversationTraceRecorder};
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, LlmChatRequest, LlmMessage,
    LlmMessageRole, LlmStreamEvent,
};
use crate::prompts::build_system_prompt;
use crate::protocol::{
    AgentApprovalStatus, AgentChatInput, AgentChatMessage, AgentChatOutput, AgentCommandPermission,
    AgentContextWindowPhase, AgentContextWindowSnapshot, AgentError, AgentEvent,
    AgentExtensionSnapshot, AgentPatchPermission, AgentPromptPreferences, AgentProposedAction,
    AgentResult, AgentRunContext, AgentRunStatus, AgentToolCall, AgentToolDefinition,
    AgentToolResult,
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
use extensions::{ModelRequestContext, RuntimeEffect, RuntimeExtensionEvent, RuntimeExtensions};
use file_transactions::{
    FileTransactionRunGuard, FileTransactionState, MAX_RESPONSE_FENCE_CORRECTIONS,
};
use serde_json::{json, Value};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::sync::Mutex;
use tool_flow::{
    approve_proposed_action, build_tool_observation_message, cancelled_output, done_event,
    execute_host_action_on_blocking_thread, execute_tool_on_blocking_thread,
    extract_reason_from_args, failed_tool_call_result, file_draft_from_tool_result,
    generate_run_id, llm_image_message_from_tool_result, redact_tool_call_for_event,
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
    patch_auto_approve: bool,
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

pub type AgentEventEmitter = Arc<dyn Fn(AgentEvent) + Send + Sync + 'static>;
pub type AgentConversationTraceObserver = Arc<
    dyn Fn(ConversationTraceSnapshot) -> AgentResult<Option<AgentContextBaseline>>
        + Send
        + Sync
        + 'static,
>;
pub type AgentHostActionExecutor = Arc<
    dyn Fn(AgentProposedAction, AgentCancellationToken) -> AgentResult<AgentToolResult>
        + Send
        + Sync
        + 'static,
>;

/// Optional capabilities supplied by the process that hosts the agent runtime.
///
/// Keeping these dependencies in one value prevents the runtime entry point from growing a new
/// positional parameter for every durable-state or orchestration capability.
#[derive(Clone, Default)]
pub struct AgentRuntimeHostServices {
    host_executor: Option<AgentHostActionExecutor>,
    storage: Option<Arc<StorageService>>,
    trace_observer: Option<AgentConversationTraceObserver>,
    context_compaction_services: Option<AgentContextCompactionServices>,
}

impl AgentRuntimeHostServices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_host_actions(
        mut self,
        host_executor: AgentHostActionExecutor,
        storage: Arc<StorageService>,
    ) -> Self {
        self.host_executor = Some(host_executor);
        self.storage = Some(storage);
        self
    }

    pub fn with_storage(mut self, storage: Arc<StorageService>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_trace_observer(mut self, observer: AgentConversationTraceObserver) -> Self {
        self.trace_observer = Some(observer);
        self
    }

    pub fn with_context_compaction(mut self, services: AgentContextCompactionServices) -> Self {
        self.context_compaction_services = Some(services);
        self
    }
}
pub use context_compaction::{
    AgentContextCompactionCommitOutcome, AgentContextCompactionCommitRequest,
    AgentContextCompactionGenerationOutput, AgentContextCompactionGenerationRequest,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionPrepareRequest,
    AgentContextCompactionServices,
};
pub use context_compaction_model::AgentContextCompactionModelGenerator;

pub async fn send_chat(input: AgentChatInput) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default().send_chat(input).await
}

pub async fn send_chat_with_events(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
) -> AgentResult<AgentChatOutput> {
    send_chat_with_events_and_cancellation(input, run_id, emitter, AgentCancellationToken::new())
        .await
}

pub async fn send_chat_with_events_and_cancellation(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            None,
        )
        .await
}

pub async fn send_chat_with_host_executor(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
    host_executor: AgentHostActionExecutor,
    storage: Arc<StorageService>,
) -> AgentResult<AgentChatOutput> {
    send_chat_with_host_services(
        input,
        run_id,
        emitter,
        cancellation_token,
        AgentRuntimeHostServices::new().with_host_actions(host_executor, storage),
    )
    .await
}

pub async fn send_chat_with_host_services(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
    host_services: AgentRuntimeHostServices,
) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            Some(host_services),
        )
        .await
}

pub fn next_run_id() -> String {
    generate_run_id()
}

pub fn inspect_context_window(
    input: AgentChatInput,
) -> AgentResult<Option<AgentContextWindowSnapshot>> {
    let mut state = create_conversation_context_state(input)?;
    Ok(Some(state.snapshot(AgentContextWindowPhase::Idle)))
}

pub fn conversation_context_configuration_revision(input: &AgentChatInput) -> AgentResult<String> {
    Ok(prepare_conversation_context(input)?.configuration_revision)
}

pub fn create_conversation_context_state(
    input: AgentChatInput,
) -> AgentResult<AgentConversationContextState> {
    let prepared = prepare_conversation_context(&input)?;
    let frame = assemble_context_preview(
        input.context_compaction_summary.clone(),
        input.messages,
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        &prepared.tool_definitions,
    )?;
    let detector = ContextCapacityDetector::for_model(
        &input.model,
        prepared.api_style,
        &prepared.tool_definitions,
    );
    Ok(AgentConversationContextState::new(
        prepared.configuration_revision,
        input.model,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
        detector,
        frame,
    ))
}

struct PreparedConversationContext {
    tool_definitions: Vec<AgentToolDefinition>,
    api_style: crate::protocol::AgentApiStyle,
    configuration_revision: String,
}

fn prepare_conversation_context(
    input: &AgentChatInput,
) -> AgentResult<PreparedConversationContext> {
    let run_id = "conversation-context-state";
    let PreparedRuntimeCapabilities {
        tool_definitions, ..
    } = prepare_runtime_capabilities(input, run_id, &[], true)?;
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let configuration_revision = conversation_context_configuration_revision_from_parts(
        input,
        api_style,
        &tool_definitions,
    )?;
    Ok(PreparedConversationContext {
        tool_definitions,
        api_style,
        configuration_revision,
    })
}

fn conversation_context_configuration_revision_from_parts(
    input: &AgentChatInput,
    api_style: crate::protocol::AgentApiStyle,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<String> {
    let system_prompt = build_system_prompt(
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        tool_definitions,
    );
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "model": input.model.trim(),
        "apiStyle": api_style,
        "contextWindowTokens": input.context_window_tokens,
        "reservedOutputTokens": sanitize_max_tokens(input.max_tokens),
        "systemPrompt": system_prompt,
        "toolDefinitions": tool_definitions,
    }))
    .map_err(|error| AgentError::new(format!("无法生成上下文计量配置指纹：{error}")))?;
    Ok(content_revision(&material))
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
            context_compaction_services,
        } = host_services.unwrap_or_default();
        let run_id = run_id.unwrap_or_else(generate_run_id);
        let context = input.context.clone();
        let trace_conversation_id = context
            .as_ref()
            .and_then(|context| context.conversation_id.clone());
        let trace_assistant_message_id = input.assistant_message_id.clone();
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
            patch_auto_approve,
        } = prepare_runtime_capabilities(
            &input,
            &run_id,
            extension_snapshots,
            host_executor.is_some(),
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
        let tool_context = ToolExecutionContext::from_run_context(context.as_ref())
            .with_cancellation(cancellation_token.clone())
            .with_runtime_services(run_id.clone(), storage);
        event_stream.emit(state_event(
            &run_id,
            AgentRunStatus::Running,
            Some(run_id.clone()),
            None,
        ));
        let mut usage = None;
        let mut finish_reason = None;
        let mut response_fence_corrections = 0_usize;
        let context_capacity_detector = context_window_configured.then(|| {
            ContextCapacityDetector::for_model(
                &llm_request.model,
                llm_request.api_style,
                &llm_request.tools,
            )
        });
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
                    let request_context = loop {
                        let mut request_context = active_context.clone();
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
                        if let Some(detector) = &context_capacity_detector {
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
                                    if executor.is_applicable(
                                        &compaction_plan,
                                        &run_id,
                                        trace_conversation_id.as_deref(),
                                        trace_assistant_message_id.as_deref(),
                                        visible_trace_item_count,
                                    ) {
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionStarted {
                                                run_id: run_id.clone(),
                                            },
                                        );
                                        let execution = executor
                                            .execute(
                                                &compaction_plan,
                                                &run_id,
                                                trace_conversation_id.as_deref(),
                                                trace_assistant_message_id.as_deref(),
                                                visible_trace_item_count,
                                                &cancellation_token,
                                            )
                                            .await;
                                        event_stream.emit_transient(
                                            AgentEvent::ContextCompactionFinished {
                                                run_id: run_id.clone(),
                                            },
                                        );
                                        match execution {
                                            Ok(ContextCompactionExecution::Rebase {
                                                baseline,
                                                usage: compaction_usage,
                                            }) => {
                                                merge_total_usage(&mut usage, compaction_usage);
                                                active_context = (*baseline)
                                                    .replace_persistent_context(active_context);
                                                detector.prepare_frame(&mut active_context);
                                                compaction_attempts =
                                                    compaction_attempts.saturating_add(1);
                                                continue;
                                            }
                                            Ok(ContextCompactionExecution::NotApplicable) => {}
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
                            detector.ensure_sendable(report)?;
                        }
                        break request_context;
                    };
                    let request = llm_request.request(request_context);
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
                        Ok(response) => response,
                        Err(error) if error.is_cancelled() => {
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
                        Err(error) => {
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

                    let suppressed_narration =
                        user_text_blocked && !llm_response.content.trim().is_empty();
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
                    let tool_name = tool_request.name;
                    let tool_args = tool_request.args;
                    let reason = extract_reason_from_args(&tool_args);
                    let definition_requires_approval =
                        tool_registry.requires_approval_for_call(&tool_name, &tool_args);
                    let auto_execute_command = tool_name == "run_command" && command_auto_approve;
                    let auto_execute_patch = (tool_name == "apply_patch"
                        || tool_name == "write_file")
                        && patch_auto_approve
                        && definition_requires_approval;
                    let auto_execute_host_action = auto_execute_command || auto_execute_patch;
                    let requires_approval =
                        definition_requires_approval && !auto_execute_host_action;
                    let call = AgentToolCall {
                        id: tool_request.id,
                        tool: tool_name,
                        args: tool_args,
                        approval_status: if auto_execute_host_action {
                            AgentApprovalStatus::Approved
                        } else if requires_approval {
                            AgentApprovalStatus::Required
                        } else {
                            AgentApprovalStatus::NotRequired
                        },
                        reason: reason.or_else(|| Some("agent requested tool call".to_string())),
                    };
                    conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_tool_call(&call);
                    pending_trace_baseline =
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    event_stream.emit(AgentEvent::ToolCall {
                        run_id: run_id.clone(),
                        call: redact_tool_call_for_event(&call),
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
                        let action = match tool_registry.proposed_action(&tool_context, &call) {
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

                    let result_result = if auto_execute_host_action {
                        match tool_registry.proposed_action(&tool_context, &call) {
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
                    conversation_trace
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .record_tool_result(&call, &llm_result);
                    pending_trace_baseline =
                        publish_trace_snapshot(&conversation_trace, trace_observer.as_ref())?;
                    if cancellation_token.is_cancelled()
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
                    let event_result = redact_tool_result_for_event(&result);
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
                    for effect in runtime_extensions
                        .on_event(RuntimeExtensionEvent::ToolCompleted { result: &result })?
                    {
                        match effect {
                            RuntimeEffect::EmitEvent(event) => event_stream.emit(event),
                        }
                    }

                    active_context.push(ContextItem::tool_result(
                        call.id.clone(),
                        build_tool_observation_message(&llm_result),
                        !result.ok,
                        ContextMetadata::new(
                            ContextSource::ToolResult,
                            ContextScope::Run,
                            ContextRetention::Retained,
                        )
                        .with_group(tool_exchange_group.clone()),
                    ));
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

fn publish_trace_recorder_snapshot(
    recorder: &ConversationTraceRecorder,
    observer: Option<&AgentConversationTraceObserver>,
) -> AgentResult<Option<AgentContextBaseline>> {
    if let Some(observer) = observer {
        return observer(recorder.snapshot());
    }
    Ok(None)
}

fn publish_trace_snapshot(
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    observer: Option<&AgentConversationTraceObserver>,
) -> AgentResult<Option<AgentContextBaseline>> {
    if observer.is_none() {
        return Ok(None);
    }
    let recorder = recorder.lock().unwrap_or_else(|error| error.into_inner());
    publish_trace_recorder_snapshot(&recorder, observer)
}

fn finalize_runtime_trace(
    result: AgentResult<AgentChatOutput>,
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
) -> AgentResult<AgentChatOutput> {
    let Some(conversation_id) = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return result;
    };
    let Some(assistant_message_id) = assistant_message_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return result;
    };

    match result {
        Ok(mut output) => {
            let terminal_status = match output.status {
                AgentRunStatus::Completed => Some(ConversationTurnTraceTerminalStatus::Completed),
                AgentRunStatus::Failed => Some(ConversationTurnTraceTerminalStatus::Failed),
                AgentRunStatus::Cancelled => Some(ConversationTurnTraceTerminalStatus::Cancelled),
                AgentRunStatus::Idle
                | AgentRunStatus::Running
                | AgentRunStatus::WaitingForApproval => None,
            };
            if let Some(terminal_status) = terminal_status {
                output.conversation_turn_trace = Some(finish_trace(
                    recorder,
                    run_id,
                    conversation_id,
                    assistant_message_id,
                    terminal_status,
                    None,
                ));
            }
            Ok(output)
        }
        Err(error) => {
            let message = error.to_string();
            let trace = finish_trace(
                recorder,
                run_id,
                conversation_id,
                assistant_message_id,
                ConversationTurnTraceTerminalStatus::Failed,
                Some(&message),
            );
            Err(error.with_conversation_turn_trace(trace))
        }
    }
}

fn conversation_trace_from_input_checkpoint(input: &AgentChatInput) -> ConversationTraceRecorder {
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return ConversationTraceRecorder::default();
    };
    let mut recorder = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    if let Some(continuation) = input.tool_continuation.as_ref() {
        recorder.record_tool_call(&continuation.call);
        recorder.record_tool_result(&continuation.call, &continuation.result);
    }
    recorder
}

fn attach_failed_runtime_trace(
    error: AgentError,
    recorder: &ConversationTraceRecorder,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
) -> AgentError {
    let (Some(conversation_id), Some(assistant_message_id)) = (
        conversation_id
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        assistant_message_id
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) else {
        return error;
    };
    let message = error.to_string();
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Failed,
        Some(&message),
    );
    debug_assert!(trace.validate().is_ok());
    error.with_conversation_turn_trace(trace)
}

fn finish_trace(
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
) -> ConversationTurnTrace {
    let trace = recorder
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .finish(
            run_id,
            conversation_id,
            assistant_message_id,
            terminal_status,
            terminal_error,
        );
    debug_assert!(trace.validate().is_ok());
    trace
}

struct AgentEventStream {
    events: Vec<AgentEvent>,
    emitter: Option<AgentEventEmitter>,
}

impl AgentEventStream {
    fn new(emitter: Option<AgentEventEmitter>) -> Self {
        Self {
            events: Vec::new(),
            emitter,
        }
    }

    fn emit(&mut self, event: AgentEvent) {
        if let Some(emitter) = &self.emitter {
            emitter(event.clone());
        }
        self.events.push(event);
    }

    fn emit_transient(&self, event: AgentEvent) {
        if let Some(emitter) = &self.emitter {
            emitter(event);
        }
    }

    fn into_events(self) -> Vec<AgentEvent> {
        self.events
    }
}

fn emit_tool_input_preview(
    event_stream: &mut AgentEventStream,
    run_id: &str,
    preview: crate::tools::ToolInputStreamPreview,
) {
    match preview {
        crate::tools::ToolInputStreamPreview::FileWrite(preview) => {
            event_stream.emit_transient(AgentEvent::FileWritePreviewUpdated {
                run_id: run_id.to_string(),
                preview,
            });
        }
    }
}

fn emit_context_manifest_if_enabled(
    run_id: &str,
    request_index: usize,
    context: &ContextFrame,
    tools: &[AgentToolDefinition],
) {
    if !context_diagnostics_enabled() {
        return;
    }

    let tool_definition_characters = tools
        .iter()
        .filter_map(|tool| serde_json::to_string(tool).ok())
        .map(|tool| tool.chars().count())
        .sum::<usize>();
    let manifest = json!({
        "items": context.manifest().entries,
        "toolDefinitions": {
            "count": tools.len(),
            "serializedCharacterCount": tool_definition_characters,
        }
    });
    match serde_json::to_string(&manifest) {
        Ok(manifest) => {
            eprintln!("[context-manifest] run={run_id} request={request_index} {manifest}")
        }
        Err(error) => eprintln!(
            "[context-manifest] run={run_id} request={request_index} serialization_error={error}"
        ),
    }
}

fn emit_context_budget_if_enabled(
    run_id: &str,
    request_index: usize,
    report: &ContextBudgetReport,
    compaction_query: &ContextCompactionQuery,
    compaction_plan: &ContextCompactionPlan,
) {
    if !context_diagnostics_enabled() {
        return;
    }

    let diagnostic = json!({
        "capacity": report,
        "compactionQuery": compaction_query,
        "compactionPlan": compaction_plan,
    });
    match serde_json::to_string(&diagnostic) {
        Ok(diagnostic) => {
            eprintln!("[context-budget] run={run_id} request={request_index} {diagnostic}")
        }
        Err(error) => eprintln!(
            "[context-budget] run={run_id} request={request_index} serialization_error={error}"
        ),
    }
}

fn context_diagnostics_enabled() -> bool {
    std::env::var("MYCOPILOT_CONTEXT_MANIFEST")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

fn prepare_runtime_capabilities(
    input: &AgentChatInput,
    run_id: &str,
    extension_snapshots: &[AgentExtensionSnapshot],
    host_actions_available: bool,
) -> AgentResult<PreparedRuntimeCapabilities> {
    let runtime_extensions = RuntimeExtensions::for_run(run_id, extension_snapshots)?;
    let mut tool_registry = ToolRegistry::defaults_with_search(input.search_config.as_ref());
    runtime_extensions.register_tools(&mut tool_registry)?;

    let context = input.context.as_ref();
    let command_permission = context
        .map(|context| context.permissions.command)
        .unwrap_or(AgentCommandPermission::RequireApproval);
    let command_auto_approve =
        command_permission == AgentCommandPermission::AutoApprove && host_actions_available;
    let patch_auto_approve = context
        .map(|context| {
            context.permissions.patch == AgentPatchPermission::AutoApprove
                && context.permissions.write != crate::protocol::AgentWritePermission::Denied
        })
        .unwrap_or(false)
        && host_actions_available;

    let mut tool_definitions = tool_registry.definitions();
    apply_permission_policy_to_tool_definitions(&mut tool_definitions, context);
    if command_auto_approve {
        if let Some(definition) = tool_definitions
            .iter_mut()
            .find(|definition| definition.name == "run_command")
        {
            definition.requires_approval = false;
            definition.description = "Run a validated shell command through the host execution layer. The current permission policy automatically approves this command request.".to_string();
        }
    }
    if patch_auto_approve {
        for definition in tool_definitions.iter_mut().filter(|definition| {
            definition.name == "apply_patch" || definition.name == "write_file"
        }) {
            definition.requires_approval = false;
            definition.description.push_str(
                " The current permission policy automatically approves the final validated file change.",
            );
        }
    }

    Ok(PreparedRuntimeCapabilities {
        runtime_extensions,
        tool_registry: Arc::new(tool_registry),
        tool_definitions,
        command_auto_approve,
        patch_auto_approve,
    })
}

fn assemble_context_preview(
    compaction_summary: Option<crate::ContextCompactionSummary>,
    messages: Vec<AgentChatMessage>,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    if compaction_summary.is_some()
        || messages.iter().any(|message| {
            matches!(message.role.trim(), "user" | "assistant")
                && !message.content.trim().is_empty()
        })
    {
        return ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: build_system_prompt(context, prompt_preferences, tool_definitions),
            compaction_summary,
            messages,
            attachments: ContextAttachments::default(),
        });
    }

    Ok(ContextFrame::new(vec![ContextItem::text(
        LlmMessageRole::System,
        build_system_prompt(context, prompt_preferences, tool_definitions),
        ContextSource::BackendSystemPrompt,
        ContextScope::Run,
        ContextRetention::Retained,
    )]))
}

fn build_llm_request(
    input: AgentChatInput,
    tool_definitions: &[AgentToolDefinition],
    restored_checkpoint: Option<RestoredRunCheckpoint>,
    shared_context_baseline: Option<AgentContextBaseline>,
) -> AgentResult<PreparedLlmRequest> {
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let template = LlmRequestTemplate {
        api_url: input.api_url.trim().to_string(),
        api_token: input.api_token.trim().to_string(),
        model: input.model.trim().to_string(),
        api_style,
        context_window_tokens: input.context_window_tokens,
        max_tokens: sanitize_max_tokens(input.max_tokens),
        temperature: sanitize_temperature(input.temperature),
        stream: input.stream.unwrap_or(false),
        tools: tool_definitions.to_vec(),
    };
    let configuration_revision = conversation_context_configuration_revision_from_parts(
        &input,
        api_style,
        tool_definitions,
    )?;
    let shared_context_baseline = shared_context_baseline
        .filter(|baseline| baseline.matches_configuration(&configuration_revision));
    let (
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        visible_trace_item_count,
    ) = match restored_checkpoint {
        Some(restored) => {
            let context = match shared_context_baseline {
                Some(baseline) => baseline.rebase_restored_frame(restored.context),
                None => restored.context,
            };
            (
                context,
                restored.next_model_request_index,
                restored.tool_batch,
                restored.conversation_trace,
                restored.visible_trace_item_count,
            )
        }
        None => {
            let attachment_context = build_attachment_context(&input.attachments)?;
            let mut context = match shared_context_baseline {
                Some(baseline) => baseline.into_frame(),
                None => assemble_initial_context(
                    input.context_compaction_summary,
                    input.messages,
                    AttachmentContext {
                        text: String::new(),
                        images: Vec::new(),
                    },
                    input.context.as_ref(),
                    input.prompt_preferences.as_ref(),
                    tool_definitions,
                )?,
            };
            append_attachment_context(&mut context, attachment_context);
            (
                context,
                0,
                ToolCallBatch::default(),
                ConversationTraceRecorder::default(),
                0,
            )
        }
    };

    Ok(PreparedLlmRequest {
        template,
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        visible_trace_item_count,
    })
}

fn append_attachment_context(frame: &mut ContextFrame, attachment_context: AttachmentContext) {
    if attachment_context.text.trim().is_empty() && attachment_context.images.is_empty() {
        return;
    }
    let mut message = LlmMessage::text(LlmMessageRole::User, attachment_context.text);
    message.images = attachment_context.images;
    frame.push(ContextItem::new(
        message,
        ContextMetadata::new(
            ContextSource::InputAttachment,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    ));
}

fn restore_input_checkpoint(
    input: &mut AgentChatInput,
    run_id: &str,
) -> AgentResult<Option<RestoredRunCheckpoint>> {
    let checkpoint = input.resume_checkpoint.take();
    let continuation = input.tool_continuation.take();
    let approval_decision = input.approval_decision.take();
    match (checkpoint, continuation, approval_decision) {
        (None, None, None) => Ok(None),
        (Some(checkpoint), Some(continuation), Some(decision)) => {
            if decision.action_id != continuation.call.id {
                return Err(AgentError::new(format!(
                    "审批决定 `{}` 与工具续跑 `{}` 不一致。",
                    decision.action_id, continuation.call.id
                )));
            }
            restore_run_checkpoint(checkpoint, run_id, &continuation).map(Some)
        }
        _ => Err(AgentError::new(
            "审批续跑必须同时提供完整运行检查点、审批决定和工具结果。",
        )),
    }
}

fn suppressed_narration_context_item() -> ContextItem {
    ContextItem::text(
        LlmMessageRole::System,
        "The text emitted alongside the preceding tool calls was not shown to the user because file transactions were unsettled. Do not assume the user saw it. Continue the transaction protocol and generate new text only after every draft has a finish or abort outcome.",
        ContextSource::RuntimeGuard,
        ContextScope::Run,
        ContextRetention::Retained,
    )
}

fn apply_permission_policy_to_tool_definitions(
    definitions: &mut Vec<AgentToolDefinition>,
    context: Option<&AgentRunContext>,
) {
    let permissions = context
        .map(|context| context.permissions)
        .unwrap_or_default();

    if permissions.write == crate::protocol::AgentWritePermission::Denied {
        definitions.retain(|definition| {
            definition.name != "apply_patch" && definition.name != "write_file"
        });
    }

    if permissions.read == crate::protocol::AgentReadPermission::All {
        for definition in definitions.iter_mut() {
            match definition.name.as_str() {
                "read_file" | "read_image" | "read_pdf" | "read_word" | "read_presentation"
                | "read_spreadsheet" => {
                    set_schema_property_description(
                        &mut definition.input_schema,
                        "path",
                        "Workspace-relative path, absolute local path, @home/@desktop/@documents/@downloads, or @attachments readPath.",
                    );
                }
                "search_code" => set_schema_property_description(
                    &mut definition.input_schema,
                    "path",
                    "Optional workspace-relative or absolute directory/file path, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                "search_files" => set_schema_property_description(
                    &mut definition.input_schema,
                    "path",
                    "Optional workspace-relative or absolute directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                "workspace_map" => set_schema_property_description(
                    &mut definition.input_schema,
                    "focusPath",
                    "Optional workspace-relative or absolute directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                _ => {}
            }
        }
    }

    if permissions.write == crate::protocol::AgentWritePermission::All {
        for definition in definitions.iter_mut().filter(|definition| {
            definition.name == "apply_patch" || definition.name == "write_file"
        }) {
            if definition.name == "apply_patch" {
                definition.description = "Create, update, or delete one text/code/config file through structured content or edits; Rust generates the unified diff. The target may be workspace-relative, absolute, or use @home/@desktop/@documents/@downloads. This works without a workspace when write access allows all locations. Do not use run_command to write files. Applying the generated diff still requires host approval.".to_string();
            } else {
                definition.description.push_str(" Targets may be workspace-relative, absolute, or use @home/@desktop/@documents/@downloads when write access allows all locations.");
            }
            set_schema_property_description(
                &mut definition.input_schema,
                "filePath",
                "Workspace-relative or absolute local file path, or @home/@desktop/@documents/@downloads.",
            );
        }
        if let Some(definition) = definitions
            .iter_mut()
            .find(|definition| definition.name == "run_command")
        {
            set_schema_property_description(
                &mut definition.input_schema,
                "cwd",
                "Workspace-relative or absolute working directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
            );
        }
    }
}

fn set_schema_property_description(schema: &mut Value, property: &str, description: &str) {
    if let Some(property_schema) = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut(property))
        .and_then(Value::as_object_mut)
    {
        property_schema.insert("description".to_string(), json!(description));
    }
}

fn assemble_initial_context(
    compaction_summary: Option<crate::ContextCompactionSummary>,
    messages: Vec<AgentChatMessage>,
    attachment_context: AttachmentContext,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: build_system_prompt(context, prompt_preferences, tool_definitions),
        compaction_summary,
        messages,
        attachments: ContextAttachments {
            text: attachment_context.text,
            images: attachment_context.images,
        },
    })
}

#[cfg(test)]
mod tests;
