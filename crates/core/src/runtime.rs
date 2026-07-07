// Rust agent core.
use crate::cancellation::AgentCancellationToken;
use crate::llm::{
    complete_chat, complete_chat_streaming, detect_api_style, LlmChatRequest, LlmImage, LlmMessage,
    LlmMessageRole, LlmToolCall,
};
use crate::prompts::build_system_prompt;
use crate::protocol::{
    AgentApprovalDecision, AgentApprovalDecisionStatus, AgentApprovalStatus, AgentChatInput,
    AgentChatMessage, AgentChatOutput, AgentCommandPermission, AgentError, AgentEvent,
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
    AgentPatchPermission, AgentPromptPreferences, AgentProposedAction, AgentResult,
    AgentRunContext, AgentRunStatus, AgentStateSnapshot, AgentToolCall, AgentToolContinuation,
    AgentToolDefinition, AgentToolResult, AgentUsage, AgentWorkspaceContext,
};
use crate::tools::{ToolExecutionContext, ToolRegistry};
use crate::usage::merge_total_usage;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_MAX_TOKENS: u32 = 30_000;
const MAX_MAX_TOKENS: u32 = 128_000;
const DEFAULT_TEMPERATURE: f32 = 0.6;
const MAX_TOOL_ITERATIONS: usize = 20;

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
        )
        .await
}

pub async fn send_chat_with_host_executor(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
    host_executor: AgentHostActionExecutor,
) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            Some(host_executor),
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
    ) -> AgentResult<AgentChatOutput> {
        let run_id = run_id.unwrap_or_else(generate_run_id);
        let context = input.context.clone();
        let tool_registry = Arc::new(ToolRegistry::read_only_defaults_with_search(
            input.search_config.as_ref(),
        ));
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
            if let Some(definition) = tool_definitions
                .iter_mut()
                .find(|definition| definition.name == "apply_patch")
            {
                definition.requires_approval = false;
                definition.description.push_str(
                    " The current permission policy automatically approves validated patches.",
                );
            }
        }
        let mut event_stream = AgentEventStream::new(emitter);
        event_stream.emit(AgentEvent::Started {
            run_id: run_id.clone(),
            tool_definitions: tool_definitions.clone(),
        });
        let llm_request = build_llm_request(input, &tool_definitions)?;
        let mut messages = llm_request.messages;
        let tool_context = ToolExecutionContext::from_run_context(context.as_ref())
            .with_cancellation(cancellation_token.clone());
        event_stream.emit(state_event(
            &run_id,
            AgentRunStatus::Running,
            Some(run_id.clone()),
            None,
        ));
        let mut usage = None;
        let mut finish_reason = None;
        let mut final_content = None;
        if cancellation_token.is_cancelled() {
            return Ok(cancelled_output(
                run_id,
                event_stream,
                tool_definitions,
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
                    usage,
                    finish_reason,
                ));
            }
            let request = LlmChatRequest {
                api_url: llm_request.api_url.clone(),
                api_token: llm_request.api_token.clone(),
                model: llm_request.model.clone(),
                api_style: llm_request.api_style,
                max_tokens: llm_request.max_tokens,
                temperature: llm_request.temperature,
                stream: llm_request.stream,
                messages: messages.clone(),
                tools: llm_request.tools.clone(),
            };
            let llm_response_result = if request.stream {
                let delta_run_id = run_id.clone();
                let delta_cancellation_token = cancellation_token.clone();
                complete_chat_streaming(request, cancellation_token.clone(), |delta| {
                    if delta_cancellation_token.is_cancelled() {
                        return;
                    }
                    if !delta.is_empty() {
                        event_stream.emit(AgentEvent::MessageDelta {
                            run_id: delta_run_id.clone(),
                            delta,
                        });
                    }
                })
                .await
            } else {
                complete_chat(request, cancellation_token.clone()).await
            };
            let llm_response = match llm_response_result {
                Ok(response) => response,
                Err(error) if error.is_cancelled() => {
                    return Ok(cancelled_output(
                        run_id,
                        event_stream,
                        tool_definitions,
                        usage,
                        finish_reason,
                    ));
                }
                Err(error) => return Err(error),
            };
            if cancellation_token.is_cancelled() {
                return Ok(cancelled_output(
                    run_id,
                    event_stream,
                    tool_definitions,
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
            if tool_requests.is_empty() {
                final_content = Some(llm_response.content);
                break;
            }

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

            messages.push(LlmMessage::assistant(
                llm_response.content.clone(),
                tool_requests.clone(),
            ));

            for tool_request in tool_requests {
                if cancellation_token.is_cancelled() {
                    return Ok(cancelled_output(
                        run_id,
                        event_stream,
                        tool_definitions,
                        usage,
                        finish_reason,
                    ));
                }
                let tool_name = tool_request.name;
                let tool_args = tool_request.args;
                let reason = extract_reason_from_args(&tool_args);
                let definition_requires_approval = tool_registry
                    .definition_for(&tool_name)
                    .map(|definition| definition.requires_approval)
                    .unwrap_or(false);
                let auto_execute_command = tool_name == "run_command" && command_auto_approve;
                let auto_execute_patch = tool_name == "apply_patch" && patch_auto_approve;
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
                    call: call.clone(),
                });
                if cancellation_token.is_cancelled() {
                    return Ok(cancelled_output(
                        run_id,
                        event_stream,
                        tool_definitions,
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
                            messages.push(LlmMessage::tool_result(
                                call.id.clone(),
                                build_tool_observation_message(&result),
                                true,
                            ));
                            continue;
                        }
                    };
                    if cancellation_token.is_cancelled() {
                        return Ok(cancelled_output(
                            run_id,
                            event_stream,
                            tool_definitions,
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

                    return Ok(AgentChatOutput {
                        content: String::new(),
                        status: AgentRunStatus::WaitingForApproval,
                        run_id,
                        events: event_stream.into_events(),
                        tool_definitions,
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
                        usage,
                        finish_reason,
                    ));
                }
                let event_result = redact_tool_result_for_event(&result);
                event_stream.emit(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: event_result.clone(),
                });

                messages.push(LlmMessage::tool_result(
                    call.id.clone(),
                    build_tool_observation_message(&event_result),
                    !result.ok,
                ));
                if let Some(image_message) = llm_image_message_from_tool_result(&result) {
                    messages.push(image_message);
                }
                if cancellation_token.is_cancelled() {
                    return Ok(cancelled_output(
                        run_id,
                        event_stream,
                        tool_definitions,
                        usage,
                        finish_reason,
                    ));
                }
            }
        }

        if cancellation_token.is_cancelled() {
            return Ok(cancelled_output(
                run_id,
                event_stream,
                tool_definitions,
                usage,
                finish_reason,
            ));
        }
        let content = final_content.unwrap_or_else(|| "没有生成可显示的回复。".to_string());
        if !llm_request.stream {
            event_stream.emit(AgentEvent::MessageDelta {
                run_id: run_id.clone(),
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

    fn into_events(self) -> Vec<AgentEvent> {
        self.events
    }
}

fn build_llm_request(
    input: AgentChatInput,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<LlmChatRequest> {
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let attachment_context = build_attachment_context(&input.attachments)?;
    let tool_continuation = input.tool_continuation.clone();
    let messages = build_runtime_messages(
        input.messages,
        attachment_context,
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        input.approval_decision.as_ref(),
        tool_continuation.as_ref(),
        tool_definitions,
    )?;

    Ok(LlmChatRequest {
        api_url: input.api_url.trim().to_string(),
        api_token: input.api_token.trim().to_string(),
        model: input.model.trim().to_string(),
        api_style,
        max_tokens: sanitize_max_tokens(input.max_tokens),
        temperature: sanitize_temperature(input.temperature),
        stream: input.stream.unwrap_or(false),
        messages,
        tools: tool_definitions.to_vec(),
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
        definitions.retain(|definition| definition.name != "apply_patch");
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
        for definition in definitions
            .iter_mut()
            .filter(|definition| definition.name == "apply_patch")
        {
            definition.description = "Create, update, or delete one text/code/config file through structured content or edits; Rust generates the unified diff. The target may be workspace-relative, absolute, or use @home/@desktop/@documents/@downloads. This works without a workspace when write access allows all locations. Do not use run_command to write files. Applying the generated diff still requires host approval.".to_string();
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

fn build_runtime_messages(
    messages: Vec<AgentChatMessage>,
    attachment_context: AttachmentContext,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    approval_decision: Option<&AgentApprovalDecision>,
    tool_continuation: Option<&AgentToolContinuation>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<Vec<LlmMessage>> {
    let mut normalized = normalize_messages(messages)?;
    append_attachment_text_to_last_user_message(&mut normalized, &attachment_context.text);

    if normalized.is_empty() {
        return Err(AgentError::new("没有可发送的对话内容。"));
    }

    if !normalized.iter().any(|message| message.role != "system") {
        return Err(AgentError::new("对话里缺少用户或助手消息。"));
    }

    if tool_continuation.is_none() {
        if let Some(approval_decision) = approval_decision {
            normalized.push(AgentChatMessage {
                role: "user".to_string(),
                content: build_approval_decision_observation(approval_decision),
            });
        }
    }

    let mut runtime_messages = normalized
        .into_iter()
        .map(agent_message_to_llm_message)
        .collect::<AgentResult<Vec<_>>>()?;
    attach_images_to_last_user_message(&mut runtime_messages, attachment_context.images);

    if let Some(continuation) = tool_continuation {
        runtime_messages.push(LlmMessage::assistant(
            "",
            vec![LlmToolCall {
                id: continuation.call.id.clone(),
                name: continuation.call.tool.clone(),
                args: continuation.call.args.clone(),
            }],
        ));
        runtime_messages.push(LlmMessage::tool_result(
            continuation.call.id.clone(),
            build_tool_observation_message(&continuation.result),
            !continuation.result.ok,
        ));
    }

    runtime_messages.insert(
        0,
        LlmMessage::text(
            LlmMessageRole::System,
            build_system_prompt(context, prompt_preferences, tool_definitions),
        ),
    );

    Ok(runtime_messages)
}

fn agent_message_to_llm_message(message: AgentChatMessage) -> AgentResult<LlmMessage> {
    let role = match message.role.as_str() {
        "system" => LlmMessageRole::System,
        "user" => LlmMessageRole::User,
        "assistant" => LlmMessageRole::Assistant,
        _ => {
            return Err(AgentError::new(format!(
                "不支持的消息角色：{}",
                message.role
            )))
        }
    };

    Ok(LlmMessage::text(role, message.content))
}

fn normalize_messages(messages: Vec<AgentChatMessage>) -> AgentResult<Vec<AgentChatMessage>> {
    let mut normalized = Vec::new();

    for message in messages {
        let role = message.role.trim();
        let content = message.content.trim();

        if content.is_empty() {
            continue;
        }

        match role {
            "system" | "user" | "assistant" => normalized.push(AgentChatMessage {
                role: role.to_string(),
                content: content.to_string(),
            }),
            _ => return Err(AgentError::new(format!("不支持的消息角色：{role}"))),
        }
    }

    Ok(normalized)
}

struct AttachmentContext {
    text: String,
    images: Vec<LlmImage>,
}

fn build_attachment_context(
    attachments: &[AgentInputAttachment],
) -> AgentResult<AttachmentContext> {
    if attachments.is_empty() {
        return Ok(AttachmentContext {
            text: String::new(),
            images: Vec::new(),
        });
    }

    let temp_root = std::env::temp_dir().join(format!(
        "my-copilot-agent-attachments-{}",
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&temp_root)
        .map_err(|error| AgentError::new(format!("创建附件临时目录失败：{error}")))?;

    let result = build_attachment_context_in_workspace(attachments, &temp_root);
    let _ = fs::remove_dir_all(&temp_root);
    result
}

fn build_attachment_context_in_workspace(
    attachments: &[AgentInputAttachment],
    temp_root: &Path,
) -> AgentResult<AttachmentContext> {
    let registry = ToolRegistry::read_only_defaults_with_search(None);
    let tool_context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("input attachments".to_string()),
            root_path: Some(temp_root.to_string_lossy().to_string()),
        }),
        attachment_library: None,
        permissions: Default::default(),
    }));
    let mut sections = Vec::new();
    let mut images = Vec::new();

    for attachment in attachments {
        let safe_name = sanitize_attachment_file_name(&attachment.name, &attachment.id);
        let mime_type = attachment
            .mime_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("application/octet-stream");

        if attachment.kind == AgentInputAttachmentKind::Image
            && attachment.encoding == AgentInputAttachmentEncoding::Base64
            && mime_type.starts_with("image/")
            && mime_type != "image/svg+xml"
        {
            images.push(LlmImage {
                mime_type: mime_type.to_string(),
                data_base64: attachment.data.clone(),
            });
            sections.push(format!(
                "### {}\n类型：图片\nMIME：{}\n大小：{} bytes\n状态：已作为视觉输入发送给模型。",
                attachment.name, mime_type, attachment.size_bytes
            ));
            continue;
        }

        let Some(tool_name) = read_tool_for_attachment(attachment, &safe_name) else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n状态：已收到附件，但当前没有适合的只读解析工具。",
                attachment.name, mime_type, attachment.size_bytes
            ));
            continue;
        };

        let file_path = temp_root.join(&safe_name);
        let bytes = attachment_bytes(attachment)?;
        fs::write(&file_path, bytes)
            .map_err(|error| AgentError::new(format!("写入附件临时文件失败：{error}")))?;

        let call = AgentToolCall {
            id: format!("attachment-{}", attachment.id),
            tool: tool_name.to_string(),
            args: json!({
                "path": safe_name,
                "maxChars": 40_000,
                "maxLines": 1_200
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("read user input attachment".to_string()),
        };
        let result = registry.execute(&tool_context, &call);

        if result.ok {
            let extracted = result
                .result
                .as_ref()
                .and_then(extracted_text_from_tool_result)
                .unwrap_or_default();
            let truncated = result
                .result
                .as_ref()
                .and_then(|value| value.get("truncated"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || attachment.truncated.unwrap_or(false);
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n读取工具：{}\n截断：{}\n\n{}",
                attachment.name, mime_type, attachment.size_bytes, tool_name, truncated, extracted
            ));
        } else {
            sections.push(format!(
                "### {}\nMIME：{}\n大小：{} bytes\n读取工具：{}\n错误：{}",
                attachment.name,
                mime_type,
                attachment.size_bytes,
                tool_name,
                result.error.unwrap_or_else(|| "附件读取失败。".to_string())
            ));
        }
    }

    let text = if sections.is_empty() {
        String::new()
    } else {
        format!(
            "用户输入框附件内容如下。附件来自用户本次输入，不是 workspace 文件；回答时可以引用这些内容，但不要声称它们已经存在于项目目录中。\n\n{}",
            sections.join("\n\n")
        )
    };

    Ok(AttachmentContext { text, images })
}

fn append_attachment_text_to_last_user_message(
    messages: &mut [AgentChatMessage],
    attachment_text: &str,
) {
    if attachment_text.trim().is_empty() {
        return;
    }

    if let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    {
        message.content = format!("{}\n\n{}", message.content, attachment_text);
    }
}

fn attach_images_to_last_user_message(messages: &mut [LlmMessage], images: Vec<LlmImage>) {
    if images.is_empty() {
        return;
    }

    if let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == LlmMessageRole::User)
    {
        message.images.extend(images);
    }
}

fn read_tool_for_attachment(
    attachment: &AgentInputAttachment,
    safe_name: &str,
) -> Option<&'static str> {
    let extension = attachment_extension(safe_name);
    match extension.as_str() {
        "pdf" => Some("read_pdf"),
        "doc" | "docx" => Some("read_word"),
        "ppt" | "pptx" => Some("read_presentation"),
        "xls" | "xlsx" | "csv" | "tsv" => Some("read_spreadsheet"),
        _ if is_text_attachment(attachment, safe_name) => Some("read_file"),
        _ => None,
    }
}

fn is_text_attachment(attachment: &AgentInputAttachment, safe_name: &str) -> bool {
    let mime_type = attachment
        .mime_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();

    attachment.encoding == AgentInputAttachmentEncoding::Utf8
        || mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json" | "application/xml" | "image/svg+xml"
        )
        || matches!(
            attachment_extension(safe_name).as_str(),
            "txt"
                | "text"
                | "md"
                | "markdown"
                | "mdx"
                | "rst"
                | "log"
                | "json"
                | "jsonl"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "cfg"
                | "conf"
                | "env"
                | "lock"
                | "properties"
                | "plist"
                | "rc"
                | "gitignore"
                | "gitattributes"
                | "editorconfig"
                | "py"
                | "pyi"
                | "ipynb"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "mjs"
                | "cjs"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "xml"
                | "sql"
                | "graphql"
                | "gql"
                | "proto"
                | "prisma"
                | "sh"
                | "bash"
                | "zsh"
                | "fish"
                | "ps1"
                | "bat"
                | "cmd"
                | "rs"
                | "go"
                | "java"
                | "kt"
                | "kts"
                | "c"
                | "h"
                | "cpp"
                | "cc"
                | "cxx"
                | "hpp"
                | "hh"
                | "hxx"
                | "cs"
                | "php"
                | "rb"
                | "swift"
                | "scala"
                | "r"
                | "m"
                | "pl"
                | "pm"
                | "lua"
                | "dart"
                | "ex"
                | "exs"
                | "erl"
                | "hrl"
                | "clj"
                | "cljs"
                | "cljc"
                | "edn"
                | "fs"
                | "fsi"
                | "fsx"
                | "elm"
                | "hs"
                | "lhs"
                | "jl"
                | "ml"
                | "mli"
                | "nim"
                | "nims"
                | "zig"
                | "v"
                | "vh"
                | "sv"
                | "svh"
                | "sol"
                | "tf"
                | "tfvars"
                | "hcl"
                | "gradle"
                | "groovy"
                | "dockerfile"
                | "cmake"
                | "make"
                | "mk"
                | "tex"
                | "bib"
                | "vue"
                | "svelte"
                | "astro"
        )
}

fn attachment_bytes(attachment: &AgentInputAttachment) -> AgentResult<Vec<u8>> {
    match attachment.encoding {
        AgentInputAttachmentEncoding::Utf8 => Ok(attachment.data.as_bytes().to_vec()),
        AgentInputAttachmentEncoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(attachment.data.as_bytes())
            .map_err(|error| AgentError::new(format!("附件 base64 数据无效：{error}"))),
    }
}

fn extracted_text_from_tool_result(value: &Value) -> Option<String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| value.get("content").and_then(Value::as_str))
        .map(ToString::to_string)
}

fn sanitize_attachment_file_name(name: &str, fallback_id: &str) -> String {
    let file_name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_id);
    let sanitized = file_name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    if sanitized.trim().is_empty() {
        fallback_id.to_string()
    } else {
        sanitized
    }
}

fn attachment_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .unwrap_or_default()
}

#[derive(Debug)]
struct ToolCallRequest {
    tool: String,
    args: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallEnvelope {
    #[serde(rename = "type")]
    kind: Option<String>,
    tool: Option<String>,
    args: Option<Value>,
    call: Option<NestedToolCallEnvelope>,
}

#[derive(Debug, Deserialize)]
struct NestedToolCallEnvelope {
    tool: String,
    args: Option<Value>,
}

fn parse_tool_call_request(content: &str) -> Option<ToolCallRequest> {
    let value = extract_json_value(content)?;
    let envelope: ToolCallEnvelope = serde_json::from_value(value).ok()?;
    let kind = envelope.kind.as_deref().unwrap_or("tool_call");
    if kind != "tool_call" {
        return None;
    }

    if let Some(call) = envelope.call {
        return Some(ToolCallRequest {
            tool: call.tool,
            args: call.args.unwrap_or_else(|| json!({})),
        });
    }

    envelope.tool.map(|tool| ToolCallRequest {
        tool,
        args: envelope.args.unwrap_or_else(|| json!({})),
    })
}

fn tool_calls_from_response(
    native_tool_calls: Vec<LlmToolCall>,
    content: &str,
    run_id: &str,
    iteration: usize,
) -> Vec<LlmToolCall> {
    if !native_tool_calls.is_empty() {
        return native_tool_calls;
    }

    parse_tool_call_request(content)
        .map(|request| {
            vec![LlmToolCall {
                id: format!("tool-{run_id}-{}", iteration + 1),
                name: request.tool,
                args: request.args,
            }]
        })
        .unwrap_or_default()
}

fn extract_json_value(content: &str) -> Option<Value> {
    let trimmed = content.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    if let Some(stripped) = strip_json_code_fence(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(stripped.trim()) {
            return Some(value);
        }
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }

    serde_json::from_str::<Value>(&trimmed[start..=end]).ok()
}

fn strip_json_code_fence(content: &str) -> Option<&str> {
    let without_start = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))?;
    without_start.strip_suffix("```")
}

fn build_tool_observation_message(result: &AgentToolResult) -> String {
    let payload = if result.ok {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": true,
            "result": result.result
        })
    } else {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": false,
            "error": result.error
        })
    };
    let payload = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());

    format!(
        "Tool result observation. Use this result to continue. Do not repeat the same tool call unless more information is needed.\n```json\n{payload}\n```"
    )
}

async fn execute_tool_on_blocking_thread(
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
    call: AgentToolCall,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let handle = tokio::task::spawn_blocking(move || registry.execute(&context, &call));
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        result = handle => {
            result.map_err(|error| AgentError::new(format!("工具执行线程失败：{error}")))
        }
    }
}

async fn execute_host_action_on_blocking_thread(
    executor: AgentHostActionExecutor,
    action: AgentProposedAction,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let execution_token = cancellation_token.clone();
    let handle = tokio::task::spawn_blocking(move || executor(action, execution_token));
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        result = handle => {
            result
                .map_err(|error| AgentError::new(format!("host 执行线程失败：{error}")))?
        }
    }
}

fn approve_proposed_action(mut action: AgentProposedAction) -> AgentProposedAction {
    match &mut action {
        AgentProposedAction::Command { command } => {
            command.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::Diff { diff } => {
            diff.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::ToolCall { call } => {
            call.approval_status = AgentApprovalStatus::Approved;
        }
    }
    action
}

fn failed_tool_call_result(call: &AgentToolCall, error: AgentError) -> AgentToolResult {
    AgentToolResult {
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: None,
        error: Some(error.to_string()),
    }
}

fn redact_tool_result_for_event(result: &AgentToolResult) -> AgentToolResult {
    let mut redacted = result.clone();
    if let Some(value) = redacted.result.as_mut() {
        redact_base64_fields(value);
    }

    redacted
}

fn redact_base64_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "dataBase64" {
                    *value = json!("[redacted]");
                } else {
                    redact_base64_fields(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_base64_fields(item);
            }
        }
        _ => {}
    }
}

fn llm_image_message_from_tool_result(result: &AgentToolResult) -> Option<LlmMessage> {
    if !result.ok || result.tool != "read_image" {
        return None;
    }

    let result_value = result.result.as_ref()?;
    let image = result_value.get("image")?;
    let mime_type = image
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let data_base64 = image
        .get("dataBase64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let path = result_value
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image");

    let mut message = LlmMessage::text(
        LlmMessageRole::User,
        format!(
            "The read_image tool returned visual input for `{path}`. Inspect the attached image before continuing."
        ),
    );
    message.images.push(LlmImage {
        mime_type: mime_type.to_string(),
        data_base64: data_base64.to_string(),
    });

    Some(message)
}

fn build_approval_decision_observation(decision: &AgentApprovalDecision) -> String {
    let status = match decision.status {
        AgentApprovalDecisionStatus::Approved => "approved",
        AgentApprovalDecisionStatus::Rejected => "rejected",
    };
    let payload = json!({
        "type": "approval_decision",
        "actionId": decision.action_id,
        "status": status,
        "message": decision.message
    });
    let payload = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());

    format!(
        "Approval decision observation. If rejected, respect the user's reason or requested alternative before continuing.\n```json\n{payload}\n```"
    )
}

fn extract_reason_from_args(args: &Value) -> Option<String> {
    args.get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(ToString::to_string)
}

fn sanitize_max_tokens(max_tokens: Option<u32>) -> u32 {
    max_tokens
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .min(MAX_MAX_TOKENS)
}

fn sanitize_temperature(temperature: Option<f32>) -> f32 {
    match temperature {
        Some(value) if value.is_finite() => value.clamp(0.0, 2.0),
        _ => DEFAULT_TEMPERATURE,
    }
}

fn state_event(
    run_id: &str,
    status: AgentRunStatus,
    active_run_id: Option<String>,
    last_error: Option<String>,
) -> AgentEvent {
    AgentEvent::State {
        run_id: run_id.to_string(),
        state: AgentStateSnapshot {
            status,
            active_run_id,
            last_error,
            updated_at: now_ms(),
        },
    }
}

fn done_event(
    run_id: &str,
    success: bool,
    status: AgentRunStatus,
    content: Option<String>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
    proposed_actions: Vec<AgentProposedAction>,
) -> AgentEvent {
    AgentEvent::Done {
        run_id: run_id.to_string(),
        success,
        status: Some(status),
        content,
        usage,
        finish_reason,
        proposed_actions,
    }
}

fn cancelled_output(
    run_id: String,
    mut event_stream: AgentEventStream,
    tool_definitions: Vec<AgentToolDefinition>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
) -> AgentChatOutput {
    event_stream.emit(state_event(&run_id, AgentRunStatus::Cancelled, None, None));
    event_stream.emit(done_event(
        &run_id,
        false,
        AgentRunStatus::Cancelled,
        None,
        usage.clone(),
        finish_reason.clone(),
        Vec::new(),
    ));

    AgentChatOutput {
        content: String::new(),
        status: AgentRunStatus::Cancelled,
        run_id,
        events: event_stream.into_events(),
        tool_definitions,
        usage,
        finish_reason,
        proposed_actions: Vec::new(),
    }
}

fn generate_run_id() -> String {
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("run-{}-{counter}", now_ms())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
        AgentRunContext, AgentWorkspaceContext,
    };

    fn message(role: &str, content: &str) -> AgentChatMessage {
        AgentChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    fn empty_attachment_context() -> AttachmentContext {
        AttachmentContext {
            text: String::new(),
            images: Vec::new(),
        }
    }

    #[test]
    fn normalizes_supported_messages_and_skips_empty_content() {
        let messages = normalize_messages(vec![
            message(" user ", " hello "),
            message("assistant", " "),
            message("system", "rules"),
        ])
        .unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
    }

    #[test]
    fn runtime_messages_add_backend_system_prompt() {
        let context = AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: Some("project-1".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-1".to_string()),
                display_name: Some("Workspace".to_string()),
                root_path: Some("/private/path".to_string()),
            }),
            attachment_library: None,
            permissions: Default::default(),
        };
        let messages = build_runtime_messages(
            vec![message("user", "Read src/main.rs")],
            empty_attachment_context(),
            Some(&context),
            None,
            None,
            None,
            &ToolRegistry::read_only_defaults_with_search(None).definitions(),
        )
        .unwrap();

        assert_eq!(messages[0].role.as_str(), "system");
        assert!(messages[0].content.contains("MyCopilot"));
        assert!(!messages[0].content.contains("/private/path"));
        assert_eq!(messages[1].role.as_str(), "user");
    }

    #[test]
    fn runtime_messages_include_approval_decision_observation() {
        let decision = AgentApprovalDecision {
            action_id: "tool-1".to_string(),
            status: AgentApprovalDecisionStatus::Rejected,
            message: Some("不要运行安装命令，先说明替代方案。".to_string()),
        };
        let messages = build_runtime_messages(
            vec![message("user", "Run pnpm install")],
            empty_attachment_context(),
            None,
            None,
            Some(&decision),
            None,
            &ToolRegistry::read_only_defaults_with_search(None).definitions(),
        )
        .unwrap();

        assert!(messages
            .iter()
            .any(|message| message.content.contains("approval_decision")
                && message.content.contains("不要运行安装命令")));
    }

    #[test]
    fn runtime_messages_include_text_attachment_content() {
        let messages = build_runtime_messages(
            vec![message("user", "Summarize this attachment")],
            AttachmentContext {
                text: "用户输入框附件内容如下。\n\n### notes.txt\nhello from attachment"
                    .to_string(),
                images: Vec::new(),
            },
            None,
            None,
            None,
            None,
            &ToolRegistry::read_only_defaults_with_search(None).definitions(),
        )
        .unwrap();

        assert!(messages
            .iter()
            .any(|message| message.role == LlmMessageRole::User
                && message.content.contains("hello from attachment")));
    }

    #[test]
    fn runtime_messages_resume_with_native_tool_call_and_result() {
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                id: "patch-1".to_string(),
                tool: "apply_patch".to_string(),
                args: json!({
                    "operation": "update",
                    "filePath": "src/main.rs",
                    "edits": [{ "kind": "append", "text": "\nfn test() {}\n" }]
                }),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                call_id: "patch-1".to_string(),
                tool: "apply_patch".to_string(),
                ok: false,
                result: None,
                error: Some("stale_file".to_string()),
            },
        };
        let messages = build_runtime_messages(
            vec![message("user", "Edit src/main.rs")],
            empty_attachment_context(),
            None,
            None,
            None,
            Some(&continuation),
            &ToolRegistry::read_only_defaults_with_search(None).definitions(),
        )
        .unwrap();

        let assistant = messages
            .iter()
            .find(|message| !message.tool_calls.is_empty())
            .unwrap();
        assert_eq!(assistant.role, LlmMessageRole::Assistant);
        assert_eq!(assistant.tool_calls[0].id, "patch-1");
        let result = messages
            .iter()
            .find(|message| message.role == LlmMessageRole::Tool)
            .unwrap();
        assert_eq!(result.tool_call_id.as_deref(), Some("patch-1"));
        assert!(result.is_error);
        assert!(result.content.contains("stale_file"));
    }

    #[test]
    fn attachment_context_reads_text_with_registered_tool() {
        let context = build_attachment_context(&[AgentInputAttachment {
            id: "attachment-1".to_string(),
            kind: AgentInputAttachmentKind::File,
            name: "notes.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 16,
            encoding: AgentInputAttachmentEncoding::Utf8,
            data: "hello from file".to_string(),
            truncated: None,
        }])
        .unwrap();

        assert!(context.text.contains("读取工具：read_file"));
        assert!(context.text.contains("hello from file"));
    }

    #[test]
    fn read_image_tool_result_is_redacted_but_creates_visual_message() {
        let result = AgentToolResult {
            call_id: "call-image".to_string(),
            tool: "read_image".to_string(),
            ok: true,
            result: Some(json!({
                "path": "@attachments/image1/pixel.png",
                "format": "png",
                "mimeType": "image/png",
                "sizeBytes": 3,
                "image": {
                    "mimeType": "image/png",
                    "dataBase64": "YWJj"
                }
            })),
            error: None,
        };

        let redacted = redact_tool_result_for_event(&result);
        assert_eq!(
            redacted.result.as_ref().unwrap()["image"]["dataBase64"],
            "[redacted]"
        );

        let image_message = llm_image_message_from_tool_result(&result).unwrap();
        assert_eq!(image_message.role, LlmMessageRole::User);
        assert_eq!(image_message.images.len(), 1);
        assert_eq!(image_message.images[0].mime_type, "image/png");
        assert_eq!(image_message.images[0].data_base64, "YWJj");
    }

    #[test]
    fn rejects_unknown_message_roles() {
        let error = normalize_messages(vec![message("tool", "result")]).unwrap_err();
        assert!(error.to_string().contains("不支持的消息角色"));
    }

    #[test]
    fn parses_plain_and_fenced_tool_calls() {
        let plain = parse_tool_call_request(
            r#"{"type":"tool_call","tool":"search_files","args":{"query":"main"}}"#,
        )
        .unwrap();
        let fenced = parse_tool_call_request(
            "```json\n{\"type\":\"tool_call\",\"tool\":\"read_file\",\"args\":{\"path\":\"src/lib.rs\"}}\n```",
        )
        .unwrap();

        assert_eq!(plain.tool, "search_files");
        assert_eq!(plain.args["query"], "main");
        assert_eq!(fenced.tool, "read_file");
        assert_eq!(fenced.args["path"], "src/lib.rs");
    }
}
