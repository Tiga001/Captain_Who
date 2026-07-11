mod attachments;
mod extensions;
mod file_transactions;
mod tool_flow;
mod tool_input_stream;

use crate::cancellation::AgentCancellationToken;
use crate::context::{
    ContextAssembler, ContextAssemblyInput, ContextAttachments, ContextFrame, ContextGroup,
    ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
    ContextToolContinuation,
};
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, LlmChatRequest, LlmMessageRole,
    LlmStreamEvent, LlmToolCall,
};
use crate::prompts::build_system_prompt;
use crate::protocol::{
    AgentApprovalDecision, AgentApprovalStatus, AgentChatInput, AgentChatMessage, AgentChatOutput,
    AgentCommandPermission, AgentError, AgentEvent, AgentPatchPermission, AgentPromptPreferences,
    AgentProposedAction, AgentResult, AgentRunContext, AgentRunStatus, AgentToolCall,
    AgentToolContinuation, AgentToolDefinition, AgentToolResult,
};
use crate::storage::service::StorageService;
use crate::tools::{ToolExecutionContext, ToolRegistry};
use crate::usage::merge_total_usage;
use attachments::{build_attachment_context, AttachmentContext};
use extensions::{ModelRequestContext, RuntimeEffect, RuntimeExtensionEvent, RuntimeExtensions};
use file_transactions::{
    FileTransactionRunGuard, FileTransactionState, MAX_RESPONSE_FENCE_CORRECTIONS,
};
use serde_json::{json, Value};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tool_flow::{
    approve_proposed_action, build_approval_decision_observation, build_tool_observation_message,
    cancelled_output, done_event, execute_host_action_on_blocking_thread,
    execute_tool_on_blocking_thread, extract_reason_from_args, failed_tool_call_result,
    file_draft_from_tool_result, generate_run_id, llm_image_message_from_tool_result,
    redact_tool_call_for_event, redact_tool_result_for_event, redact_tool_result_for_llm,
    sanitize_max_tokens, sanitize_temperature, state_event, tool_calls_from_response,
};
use tool_input_stream::ToolInputStreamObservers;

const DEFAULT_MAX_TOKENS: u32 = 30_000;
const MAX_MAX_TOKENS: u32 = 128_000;
const DEFAULT_TEMPERATURE: f32 = 0.6;
const MAX_TOOL_ITERATIONS: usize = 10_000;

struct LlmRequestTemplate {
    api_url: String,
    api_token: String,
    model: String,
    api_style: crate::protocol::AgentApiStyle,
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
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

pub type AgentEventEmitter = Arc<dyn Fn(AgentEvent) + Send + Sync + 'static>;
pub type AgentHostActionExecutor = Arc<
    dyn Fn(AgentProposedAction, AgentCancellationToken) -> AgentResult<AgentToolResult>
        + Send
        + Sync
        + 'static,
>;

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
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            Some(host_executor),
            Some(storage),
        )
        .await
}

pub fn next_run_id() -> String {
    generate_run_id()
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
            None,
        )
        .await
    }

    pub async fn send_chat_with_events_and_cancellation(
        &self,
        input: AgentChatInput,
        run_id: Option<String>,
        emitter: Option<AgentEventEmitter>,
        cancellation_token: AgentCancellationToken,
        host_executor: Option<AgentHostActionExecutor>,
        storage: Option<Arc<StorageService>>,
    ) -> AgentResult<AgentChatOutput> {
        let run_id = run_id.unwrap_or_else(generate_run_id);
        let context = input.context.clone();
        let mut runtime_extensions =
            RuntimeExtensions::for_run(&run_id, &input.extension_snapshots)?;
        let mut tool_registry = ToolRegistry::defaults_with_search(input.search_config.as_ref());
        runtime_extensions.register_tools(&mut tool_registry)?;
        let tool_registry = Arc::new(tool_registry);
        let command_permission = context
            .as_ref()
            .map(|context| context.permissions.command)
            .unwrap_or(AgentCommandPermission::RequireApproval);
        let command_auto_approve =
            command_permission == AgentCommandPermission::AutoApprove && host_executor.is_some();
        let patch_auto_approve = context
            .as_ref()
            .map(|context| {
                context.permissions.patch == AgentPatchPermission::AutoApprove
                    && context.permissions.write != crate::protocol::AgentWritePermission::Denied
            })
            .unwrap_or(false)
            && host_executor.is_some();
        let mut tool_definitions = tool_registry.definitions();
        apply_permission_policy_to_tool_definitions(&mut tool_definitions, context.as_ref());
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
        let mut event_stream = AgentEventStream::new(emitter);
        event_stream.emit(AgentEvent::Started {
            run_id: run_id.clone(),
            tool_definitions: tool_definitions.clone(),
        });
        let transaction_storage = storage.clone();
        let mut file_transaction_guard = FileTransactionRunGuard::new(
            transaction_storage.clone(),
            run_id.clone(),
            cancellation_token.clone(),
        );
        let PreparedLlmRequest {
            template: llm_request,
            context: mut active_context,
        } = build_llm_request(input, &tool_definitions)?;
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
        let mut final_content = None;
        let mut response_fence_corrections = 0_usize;
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

        for iteration in 0..=self.max_tool_iterations {
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
            let file_transactions =
                FileTransactionState::load(transaction_storage.as_deref(), &run_id)?;
            let user_text_blocked = file_transactions.blocks_user_text();
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
                iteration + 1,
                &request_context,
                &llm_request.tools,
            );
            let request = llm_request.request(request_context);
            let llm_response_result = if request.stream {
                let delta_run_id = run_id.clone();
                let stream_id = format!("{}-stream-{}", run_id, iteration + 1);
                let delta_cancellation_token = cancellation_token.clone();
                let mut tool_input_stream = ToolInputStreamObservers::default();
                complete_chat_streaming(request, cancellation_token.clone(), |stream_event| {
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
                        LlmStreamEvent::Delta(delta) if !user_text_blocked && !delta.is_empty() => {
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
                                event_stream.emit_transient(AgentEvent::ToolInputProgress {
                                    run_id: delta_run_id.clone(),
                                    stream_id: stream_id.clone(),
                                    attempt: tool_input_stream.attempt(),
                                    tool_call_index,
                                    tool_call_id: tool_call_id.clone(),
                                    tool: tool.clone(),
                                    received_bytes,
                                });
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
                            event_stream.emit_transient(AgentEvent::FileWritePreviewCleared {
                                run_id: delta_run_id.clone(),
                                stream_id: stream_id.clone(),
                                attempt: tool_input_stream.attempt(),
                            });
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
                                emit_tool_input_preview(&mut event_stream, &delta_run_id, preview);
                            }
                            if !user_text_blocked {
                                event_stream.emit(AgentEvent::MessageStreamCommitted {
                                    run_id: delta_run_id.clone(),
                                    stream_id: stream_id.clone(),
                                });
                            }
                        }
                        LlmStreamEvent::Delta(_) => {}
                    }
                })
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

            merge_total_usage(&mut usage, llm_response.usage);
            finish_reason = llm_response.finish_reason;

            let tool_requests = tool_calls_from_response(
                llm_response.tool_calls,
                &llm_response.content,
                &run_id,
                iteration,
            );
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
                final_content = Some(llm_response.content);
                break;
            }
            response_fence_corrections = 0;

            if iteration >= self.max_tool_iterations {
                let message = "工具调用次数超过限制，已停止继续执行。".to_string();
                event_stream.emit(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    message: message.clone(),
                    recoverable: false,
                });
                final_content = Some(message);
                break;
            }

            let suppressed_narration = user_text_blocked && !llm_response.content.trim().is_empty();
            let tool_exchange_group = ContextGroup::tool_exchange(format!(
                "run:{run_id}:tool-exchange:{}",
                iteration + 1
            ));
            active_context.push(ContextItem::assistant(
                if user_text_blocked {
                    String::new()
                } else {
                    llm_response.content.clone()
                },
                tool_requests.clone(),
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(tool_exchange_group.clone()),
            ));

            for tool_request in tool_requests {
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
                let tool_name = tool_request.name;
                let tool_args = tool_request.args;
                let reason = extract_reason_from_args(&tool_args);
                let definition_requires_approval =
                    tool_registry.requires_approval_for_call(&tool_name, &tool_args);
                let auto_execute_command = tool_name == "run_command" && command_auto_approve;
                let auto_execute_patch = (tool_name == "apply_patch" || tool_name == "write_file")
                    && patch_auto_approve
                    && definition_requires_approval;
                let auto_execute_host_action = auto_execute_command || auto_execute_patch;
                let requires_approval = definition_requires_approval && !auto_execute_host_action;
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
                        Ok(action) => action,
                        Err(error) => {
                            let result = failed_tool_call_result(&call, error);
                            event_stream.emit(AgentEvent::ToolResult {
                                run_id: run_id.clone(),
                                result: result.clone(),
                            });
                            active_context.push(ContextItem::tool_result(
                                call.id.clone(),
                                build_tool_observation_message(&result),
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
                    event_stream.emit(AgentEvent::ApprovalRequired {
                        run_id: run_id.clone(),
                        action: action.clone(),
                        extension_snapshots: runtime_extensions.snapshots()?,
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
                    });
                }

                let result_result = if auto_execute_host_action {
                    match tool_registry.proposed_action(&tool_context, &call) {
                        Ok(action) => {
                            let action = approve_proposed_action(action);
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
                let llm_result = redact_tool_result_for_llm(&result);
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
            if suppressed_narration {
                active_context.push(ContextItem::text(
                    LlmMessageRole::System,
                    "The text emitted alongside the preceding tool calls was not shown to the user because file transactions were unsettled. Do not assume the user saw it. Continue the transaction protocol and generate new text only after every draft has a finish or abort outcome.",
                    ContextSource::RuntimeGuard,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ));
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
        let final_file_transactions =
            FileTransactionState::load(transaction_storage.as_deref(), &run_id)?;
        if final_file_transactions.blocks_user_text() {
            return Err(AgentError::new(
                "文件事务尚未结算，不能结束当前运行或输出最终回复。",
            ));
        }
        file_transaction_guard.complete();
        let content = final_content.unwrap_or_else(|| "没有生成可显示的回复。".to_string());
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
        })
    }
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
    let enabled = std::env::var("MYCOPILOT_CONTEXT_MANIFEST")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"));
    if !enabled {
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

fn build_llm_request(
    input: AgentChatInput,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<PreparedLlmRequest> {
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let attachment_context = build_attachment_context(&input.attachments)?;
    let tool_continuation = input.tool_continuation.clone();
    let context = assemble_initial_context(
        input.messages,
        attachment_context,
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        input.approval_decision.as_ref(),
        tool_continuation.as_ref(),
        tool_definitions,
    )?;

    Ok(PreparedLlmRequest {
        template: LlmRequestTemplate {
            api_url: input.api_url.trim().to_string(),
            api_token: input.api_token.trim().to_string(),
            model: input.model.trim().to_string(),
            api_style,
            max_tokens: sanitize_max_tokens(input.max_tokens),
            temperature: sanitize_temperature(input.temperature),
            stream: input.stream.unwrap_or(false),
            tools: tool_definitions.to_vec(),
        },
        context,
    })
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
    messages: Vec<AgentChatMessage>,
    attachment_context: AttachmentContext,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    approval_decision: Option<&AgentApprovalDecision>,
    tool_continuation: Option<&AgentToolContinuation>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    let continuation = tool_continuation.map(|continuation| ContextToolContinuation {
        call: LlmToolCall {
            id: continuation.call.id.clone(),
            name: continuation.call.tool.clone(),
            args: continuation.call.args.clone(),
        },
        observation: build_tool_observation_message(&continuation.result),
        is_error: !continuation.result.ok,
    });
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: build_system_prompt(context, prompt_preferences, tool_definitions),
        messages,
        attachments: ContextAttachments {
            text: attachment_context.text,
            images: attachment_context.images,
        },
        approval_observation: approval_decision.map(build_approval_decision_observation),
        tool_continuation: continuation,
    })
}

#[cfg(test)]
mod tests;
