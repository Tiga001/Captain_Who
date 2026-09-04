fn file_change_snapshot_from_tool_result(
    storage: Option<&Arc<StorageService>>,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> AgentResult<Option<crate::protocol::AgentFileChangeSnapshot>> {
    if call.tool != "apply_patch" || !result.ok {
        return Ok(None);
    }
    let staged_action = crate::tools::apply_patch_action(&call.args)
        .is_some_and(|action| matches!(action, "begin" | "append" | "edit" | "status" | "abort"));
    if !staged_action {
        return Ok(None);
    }
    let transaction_id = result
        .result
        .as_ref()
        .and_then(|value| value.get("transactionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AgentError::new("FileChange 结果缺少当前事务标识。"))?;
    let storage = storage.ok_or_else(|| AgentError::new("FileChange 私有存储不可用。"))?;
    let record = storage
        .get_agent_file_change(transaction_id)
        .map_err(AgentError::new)?
        .ok_or_else(|| AgentError::new("FileChange 当前事务不存在。"))?;
    file_change_snapshot(&record)
        .map(Some)
        .map_err(AgentError::new)
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

        if tool_registry.identity(tool) != Some(provenance) {
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
    durable_result: AgentToolResult,
    model_observation: String,
    checkpoint_observation: String,
    durable_observation: String,
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
    let (status, outcome, dispatch_certainty, recovery) = if dispatch_started {
        (
            "outcome_unknown",
            "outcome_unknown",
            "possiblyDispatched",
            "inspectAuthoritativeStateBeforeRetry",
        )
    } else {
        (
            "failed",
            "skipped",
            "definitelyNotDispatched",
            "retryFromSafeContextBoundary",
        )
    };
    let mut details = json!({
        "type": "runtimeGuard",
        "code": "groupedTurnAborted",
        "status": status,
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
    settle_terminal_tool_batch(
        GroupedToolBatchTerminalCause::Cancelled,
        current,
        true,
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
    settle_terminal_tool_batch(
        GroupedToolBatchTerminalCause::Aborted {
            cause_code: cause.code().unwrap_or("agent.runtime_abort").to_string(),
        },
        current,
        true,
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

/// Closes the Tool Call that has already entered the durable Trace. The current call is never
/// conditional on Provider grouping: once its ToolCall item is visible, every terminal Runtime
/// path must append the matching ToolResult. Grouped providers additionally close the queued
/// suffix because their one Assistant turn cannot be replayed with unresolved sibling calls.
#[allow(clippy::too_many_arguments)]
fn settle_aborted_current_tool_call(
    cause: &AgentError,
    current: TerminalToolCallSettlement,
    settle_queued_suffix: bool,
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
    settle_terminal_tool_batch(
        GroupedToolBatchTerminalCause::Aborted {
            cause_code: cause.code().unwrap_or("agent.runtime_abort").to_string(),
        },
        Some(current),
        settle_queued_suffix,
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
fn settle_cancelled_current_tool_call(
    current: TerminalToolCallSettlement,
    settle_queued_suffix: bool,
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
    settle_terminal_tool_batch(
        GroupedToolBatchTerminalCause::Cancelled,
        Some(current),
        settle_queued_suffix,
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
fn settle_terminal_tool_batch(
    terminal_cause: GroupedToolBatchTerminalCause,
    current: Option<TerminalToolCallSettlement>,
    settle_queued_suffix: bool,
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
    let omitted_independent_suffix = if settle_queued_suffix {
        None
    } else {
        let group = staged_batch.context_group();
        let mut queued = staged_batch.clone();
        let call_ids = std::iter::from_fn(|| queued.pop_front())
            .map(|queued| queued.call.id)
            .collect::<std::collections::BTreeSet<_>>();
        group.map(|group| (group, call_ids))
    };
    let mut settlements = Vec::with_capacity(staged_batch.len().saturating_add(1));
    if let Some(current) = current {
        settlements.push(current);
    }
    if settle_queued_suffix {
        while let Some(queued) = staged_batch.pop_front() {
            settlements.push(TerminalToolCallSettlement {
                queued,
                call: None,
                announced: false,
                dispatch_started: false,
                outcome: TerminalToolCallOutcome::Synthetic,
            });
        }
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
            let observations = finalize_tool_observations(
                model_tool_result_gate,
                &call.id,
                true,
                ToolResultProjectionLanes {
                    model: &model_result,
                    checkpoint: &checkpoint_result,
                    durable: &model_result,
                },
                &archive_metadata,
                is_mcp_tool,
            )?;
            Ok::<_, AgentError>((
                result,
                checkpoint_result,
                observations.model,
                observations.checkpoint,
                observations.durable,
                archive_metadata,
            ))
        };
        let (
            result,
            durable_result,
            model_observation,
            checkpoint_observation,
            durable_observation,
            archive_metadata,
        ) = match outcome {
            TerminalToolCallOutcome::Settled(settled) => {
                let SettledTerminalToolCallOutcome {
                    result,
                    durable_result,
                    model_observation,
                    checkpoint_observation,
                    durable_observation,
                    archive_metadata,
                } = *settled;
                (
                    result,
                    durable_result,
                    model_observation,
                    checkpoint_observation,
                    durable_observation,
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
        let result_sequence =
            staged_trace.record_tool_result_with_archive(&call, &durable_result, archive_metadata);
        if let Some(sequence) = result_sequence {
            staged_trace
                .record_model_message(
                    sequence,
                    0,
                    &LlmMessage::tool_result(call.id.clone(), durable_observation, is_error),
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
    // Generic split projections treat sibling calls independently. On a terminal Runtime error
    // those siblings were never announced or dispatched, so remove them from the live Assistant
    // turn instead of fabricating ToolResults. Exact grouped providers keep the full turn and
    // settle every suffix call above.
    if let Some((group, call_ids)) = omitted_independent_suffix {
        if !call_ids.is_empty() {
            staged_context.omit_runtime_tool_calls_from_group(&group, &call_ids)?;
        }
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

