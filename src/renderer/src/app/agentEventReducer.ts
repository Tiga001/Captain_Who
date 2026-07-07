// Renderer UI.
import type {
  AgentActionExecutionOutput,
  AgentApprovalStatus,
  AgentChatOutput,
  AgentEvent,
  AgentProposedAction,
  AgentToolCall,
  AgentToolResult,
} from "@mycopilot/protocol";
import type {
  ChatAgentRunView,
  ChatAgentTimelineItem,
  ChatMessage,
} from "../features/chat/chatTypes";
import { THINKING_PLACEHOLDER } from "./appConstants";
import { getAgentActionId } from "./agentActionUtils";
import {
  normalizeReadActivities,
  upsertReadActivityFromCall,
  upsertReadActivityFromResult,
} from "../features/chat/agentReadActivities";
import {
  normalizeWebSearchActivities,
  upsertWebSearchActivityFromCall,
  upsertWebSearchActivityFromResult,
} from "../features/chat/agentWebSearch";

function createAgentRun(
  runId: string | null,
  status: ChatAgentRunView["status"] = "starting",
): ChatAgentRunView {
  const now = Date.now();

  return {
    runId,
    status,
    startedAt: now,
    completedAt: isCompletedAgentRunStatus(status) ? now : undefined,
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    webSearchActivities: [],
    readActivities: [],
    timeline: [],
  };
}

export function ensureAgentRun(
  currentRun: ChatAgentRunView | undefined,
  runId: string | null | undefined,
  status?: ChatAgentRunView["status"],
): ChatAgentRunView {
  if (!currentRun) {
    return createAgentRun(runId ?? null, status);
  }

  return {
    ...currentRun,
    runId: currentRun.runId ?? runId ?? null,
    status: status ?? currentRun.status,
    startedAt: currentRun.startedAt ?? Date.now(),
    completedAt: isCompletedAgentRunStatus(status) ? currentRun.completedAt ?? Date.now() : currentRun.completedAt,
    timeline: currentRun.timeline ?? [],
    readActivities: currentRun.readActivities ?? [],
  };
}

function isCompletedAgentRunStatus(status: ChatAgentRunView["status"] | undefined) {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function isFinishedAgentOutputStatus(status: ChatAgentRunView["status"] | undefined) {
  return status !== "starting" && status !== "running" && status !== "waiting_for_approval";
}

function upsertById<T>(items: T[], nextItem: T, getId: (item: T) => string) {
  const nextId = getId(nextItem);
  const itemIndex = items.findIndex((item) => getId(item) === nextId);

  if (itemIndex === -1) {
    return [...items, nextItem];
  }

  return items.map((item, index) => (index === itemIndex ? nextItem : item));
}

function upsertAgentAction(actions: AgentProposedAction[], nextAction: AgentProposedAction) {
  return upsertById(actions, nextAction, getAgentActionId);
}

function removeAgentAction(actions: AgentProposedAction[], actionId: string) {
  return actions.filter((action) => getAgentActionId(action) !== actionId);
}

export function appendTimelineItem(
  run: ChatAgentRunView,
  item: ChatAgentTimelineItem,
): ChatAgentTimelineItem[] {
  if (run.timeline.some((timelineItem) => timelineItem.id === item.id)) {
    return run.timeline.map((timelineItem) => (timelineItem.id === item.id ? item : timelineItem));
  }

  return [...run.timeline, item];
}

function removeTransientToolTimelineItems(timeline: ChatAgentTimelineItem[]) {
  return timeline;
}

function appendToolCallToTimeline(
  run: ChatAgentRunView,
  callId: string,
): ChatAgentTimelineItem[] {
  return appendTimelineItem(
    {
      ...run,
      timeline: removeTransientToolTimelineItems(run.timeline),
    },
    {
      id: `tool-call-${callId}`,
      type: "tool_call",
      callId,
    },
  );
}

function getActionToolCall(action: AgentProposedAction): AgentToolCall | null {
  if (action.type === "tool_call") return action.call;

  if (action.type === "diff") {
    return {
      id: action.diff.id,
      tool: "apply_patch",
      args: {
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        patch: action.diff.patch,
        summary: action.diff.summary,
      },
      approvalStatus: action.diff.approvalStatus,
      reason: action.diff.summary,
    };
  }

  if (action.type !== "command") return null;

  return {
    id: action.command.id,
    tool: "run_command",
    args: {
      command: action.command.command,
      cwd: action.command.cwd,
      timeoutMs: action.command.timeoutMs,
      riskLevel: action.command.riskLevel,
      reason: action.command.reason,
    },
    approvalStatus: action.command.approvalStatus,
    reason: action.command.reason,
  };
}

function withActionApprovalStatus(
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus,
): AgentProposedAction {
  if (action.type === "command") {
    return {
      ...action,
      command: {
        ...action.command,
        approvalStatus,
      },
    };
  }

  if (action.type === "tool_call") {
    return {
      ...action,
      call: {
        ...action.call,
        approvalStatus,
      },
    };
  }

  return {
    ...action,
    diff: {
      ...action.diff,
      approvalStatus,
    },
  };
}

function updateToolCallApprovalStatus(
  toolCalls: AgentToolCall[],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus,
) {
  const actionId = getAgentActionId(action);
  const actionCall = getActionToolCall(withActionApprovalStatus(action, approvalStatus));

  if (actionCall) {
    return upsertById(
      toolCalls.map((call) =>
        call.id === actionId
          ? {
              ...call,
              approvalStatus,
            }
          : call,
      ),
      actionCall,
      (call) => call.id,
    );
  }

  return toolCalls;
}

function updateDiffApprovalStatus(
  diffs: ChatAgentRunView["diffs"],
  action: AgentProposedAction,
  approvalStatus: AgentApprovalStatus,
) {
  if (action.type !== "diff") return diffs;

  return upsertById(
    diffs.map((diff) =>
      diff.id === action.diff.id
        ? {
            ...diff,
            approvalStatus,
          }
        : diff,
    ),
    {
      ...action.diff,
      approvalStatus,
    },
    (diff) => diff.id,
  );
}

function createRejectedToolResult(
  action: AgentProposedAction,
  message?: string,
): AgentToolResult | null {
  const call = getActionToolCall(action);
  if (!call) return null;

  if (action.type === "diff") {
    return {
      callId: call.id,
      tool: "apply_patch",
      ok: true,
      result: {
        status: "rejected",
        operation: action.diff.operation,
        filePath: action.diff.filePath,
        appliedFilePaths: [],
        message,
      },
    };
  }

  return {
    callId: call.id,
    tool: call.tool,
    ok: true,
    result: {
      status: "rejected",
      message,
    },
  };
}

function getApprovalsForStatus(
  status: ChatAgentRunView["status"],
  currentApprovals: AgentProposedAction[],
  proposedActions: AgentProposedAction[],
) {
  if (status !== "waiting_for_approval") return [];
  return proposedActions.reduce(upsertAgentAction, currentApprovals);
}

function appendMessageDeltaToTimeline(
  run: ChatAgentRunView,
  delta: string,
): ChatAgentTimelineItem[] {
  const lastItem = run.timeline[run.timeline.length - 1];

  if (lastItem?.type === "message") {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === "message"
        ? {
            ...timelineItem,
            content: `${timelineItem.content}${delta}`,
          }
        : timelineItem,
    );
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: "message",
      content: delta,
    },
  ];
}

function appendMessageToTimeline(run: ChatAgentRunView, content: string): ChatAgentTimelineItem[] {
  const lastItem = run.timeline[run.timeline.length - 1];

  if (lastItem?.type === "message") {
    return run.timeline.map((timelineItem) =>
      timelineItem.id === lastItem.id && timelineItem.type === "message"
        ? {
            ...timelineItem,
            content,
          }
        : timelineItem,
    );
  }

  return [
    ...run.timeline,
    {
      id: `message-${run.timeline.length + 1}`,
      type: "message",
      content,
    },
  ];
}

function getMessageContentAfterDelta(content: string, delta: string) {
  const previousContent = content === THINKING_PLACEHOLDER ? "/protocol" : content;
  return `${previousContent}${delta}`;
}

function getFinalMessageContent(currentContent: string, finalContent?: string) {
  if (!finalContent) {
    return currentContent === THINKING_PLACEHOLDER ? "/protocol" : currentContent;
  }

  if (!currentContent || currentContent === THINKING_PLACEHOLDER) {
    return finalContent;
  }

  return currentContent;
}

function getRunResponseTimestamps(run: ChatAgentRunView, receivedAt: number) {
  return {
    firstResponseAt: run.firstResponseAt ?? receivedAt,
    lastResponseAt: receivedAt,
  };
}

export function shouldTouchConversationForAgentEvent(agentEvent: AgentEvent) {
  if (agentEvent.type === "done") return true;
  if (agentEvent.type === "approval_required") return true;
  if (agentEvent.type === "error" && !agentEvent.recoverable) return true;

  if (agentEvent.type === "state") {
    return ["waiting_for_approval", "completed", "failed", "cancelled"].includes(agentEvent.state.status);
  }

  return false;
}

function getChatMessageStatusFromAgentStatus(status: AgentChatOutput["status"]): ChatMessage["status"] {
  if (status === "running" || status === "waiting_for_approval") return "pending";
  if (status === "failed") return "error";
  return "sent";
}

export function applyAgentEventToChatMessage(message: ChatMessage, agentEvent: AgentEvent): ChatMessage {
  const runId = agentEvent.runId ?? message.agentRun?.runId ?? null;
  const currentRun = ensureAgentRun(message.agentRun, runId);

  if (agentEvent.type === "started") {
    return {
      ...message,
      status: "pending",
      agentRun: {
        ...currentRun,
        runId: agentEvent.runId,
        status: "running",
        startedAt: currentRun.startedAt ?? Date.now(),
        toolDefinitions: agentEvent.toolDefinitions,
      },
    };
  }

  if (agentEvent.type === "state") {
    return {
      ...message,
      status: getChatMessageStatusFromAgentStatus(agentEvent.state.status),
      agentRun: {
        ...currentRun,
        status: agentEvent.state.status,
        completedAt: isCompletedAgentRunStatus(agentEvent.state.status)
          ? currentRun.completedAt ?? Date.now()
          : currentRun.completedAt,
        state: agentEvent.state,
        error: agentEvent.state.lastError ?? currentRun.error,
      },
    };
  }

  if (agentEvent.type === "message_delta") {
    const receivedAt = Date.now();

    return {
      ...message,
      content: getMessageContentAfterDelta(message.content, agentEvent.delta),
      status: "pending",
      agentRun: {
        ...currentRun,
        status: "running",
        ...getRunResponseTimestamps(currentRun, receivedAt),
        timeline: appendMessageDeltaToTimeline(currentRun, agentEvent.delta),
      },
    };
  }

  if (agentEvent.type === "message") {
    const receivedAt = Date.now();

    return {
      ...message,
      content: agentEvent.content,
      status: "pending",
      agentRun: {
        ...currentRun,
        status: "running",
        ...getRunResponseTimestamps(currentRun, receivedAt),
        timeline: appendMessageToTimeline(currentRun, agentEvent.content),
      },
    };
  }

  if (agentEvent.type === "tool_call") {
    const runWithCleanTimeline = {
      ...currentRun,
      timeline: removeTransientToolTimelineItems(currentRun.timeline),
    };

    return {
      ...message,
      status: "pending",
      agentRun: {
        ...runWithCleanTimeline,
        status: "running",
        toolCalls: upsertById(currentRun.toolCalls, agentEvent.call, (call) => call.id),
        webSearchActivities: upsertWebSearchActivityFromCall(currentRun, agentEvent.call),
        readActivities: upsertReadActivityFromCall(currentRun, agentEvent.call),
        timeline: appendToolCallToTimeline(runWithCleanTimeline, agentEvent.call.id),
      },
    };
  }

  if (agentEvent.type === "tool_result") {
    return {
      ...message,
      status: "pending",
      agentRun: {
        ...currentRun,
        status: "running",
        toolResults: upsertById(currentRun.toolResults, agentEvent.result, (result) => result.callId),
        webSearchActivities: upsertWebSearchActivityFromResult(currentRun, agentEvent.result),
        readActivities: upsertReadActivityFromResult(currentRun, agentEvent.result),
      },
    };
  }

  if (agentEvent.type === "approval_required") {
    const call = getActionToolCall(agentEvent.action);
    const timeline = call
      ? appendToolCallToTimeline(currentRun, call.id)
      : removeTransientToolTimelineItems(currentRun.timeline);

    return {
      ...message,
      status: "pending",
      agentRun: {
        ...currentRun,
        status: "waiting_for_approval",
        toolCalls: call
          ? upsertById(currentRun.toolCalls, call, (candidate) => candidate.id)
          : currentRun.toolCalls,
        diffs: updateDiffApprovalStatus(currentRun.diffs, agentEvent.action, "required"),
        approvals: upsertAgentAction(currentRun.approvals, agentEvent.action),
        timeline,
      },
    };
  }

  if (agentEvent.type === "diff") {
    const call = getActionToolCall({ type: "diff", diff: agentEvent.diff });
    const runWithDiff = {
      ...currentRun,
      diffs: upsertById(currentRun.diffs, agentEvent.diff, (diff) => diff.id),
      toolCalls: call
        ? upsertById(currentRun.toolCalls, call, (candidate) => candidate.id)
        : currentRun.toolCalls,
    };

    return {
      ...message,
      status: "pending",
      agentRun: {
        ...runWithDiff,
        status: currentRun.status === "waiting_for_approval" ? currentRun.status : "running",
        timeline: call
          ? appendToolCallToTimeline(runWithDiff, call.id)
          : currentRun.timeline,
      },
    };
  }

  if (agentEvent.type === "command_output") {
    return message;
  }

  if (agentEvent.type === "error") {
    return {
      ...message,
      content: agentEvent.recoverable ? message.content : agentEvent.message,
      status: agentEvent.recoverable ? message.status : "error",
      agentRun: {
        ...currentRun,
        status: agentEvent.recoverable ? currentRun.status : "failed",
        completedAt: agentEvent.recoverable ? currentRun.completedAt : currentRun.completedAt ?? Date.now(),
        error: agentEvent.message,
        timeline: appendTimelineItem(currentRun, {
          id: `error-${currentRun.timeline.length + 1}`,
          type: "error",
          message: agentEvent.message,
        }),
      },
    };
  }

  const nextStatus = agentEvent.status ?? (agentEvent.success ? "completed" : "failed");
  const proposedActions = agentEvent.proposedActions ?? [];
  const completedAt = isFinishedAgentOutputStatus(nextStatus)
    ? currentRun.completedAt ?? Date.now()
    : currentRun.completedAt;
  const finalResponseAt =
    agentEvent.content && !currentRun.firstResponseAt ? completedAt ?? Date.now() : currentRun.firstResponseAt;

  return {
    ...message,
    content: getFinalMessageContent(message.content, agentEvent.content),
    status:
      nextStatus === "waiting_for_approval"
        ? "pending"
        : nextStatus === "cancelled" || agentEvent.success
          ? "sent"
          : "error",
    agentRun: {
      ...currentRun,
      status: nextStatus,
      firstResponseAt: finalResponseAt,
      lastResponseAt: agentEvent.content ? currentRun.lastResponseAt ?? finalResponseAt : currentRun.lastResponseAt,
      completedAt,
      usage: agentEvent.usage ?? currentRun.usage,
      finishReason: agentEvent.finishReason,
      approvals: getApprovalsForStatus(nextStatus, currentRun.approvals, proposedActions),
      timeline: removeTransientToolTimelineItems(currentRun.timeline),
    },
  };
}

export function applyAgentOutputToChatMessage(message: ChatMessage, output: AgentChatOutput): ChatMessage {
  const streamedContentEvents = output.events.some(
    (event) => event.type === "message" || event.type === "message_delta",
  );
  const messageWithEvents = output.events.reduce(applyAgentEventToChatMessage, message);
  const currentRun = ensureAgentRun(messageWithEvents.agentRun, output.runId, output.status);
  const nextContent =
    output.content && (!streamedContentEvents || !messageWithEvents.content || messageWithEvents.content === THINKING_PLACEHOLDER)
      ? output.content
      : messageWithEvents.content;
  const outputCompletedAt = isFinishedAgentOutputStatus(output.status)
    ? currentRun.completedAt ?? Date.now()
    : currentRun.completedAt;
  const finalResponseAt =
    nextContent && nextContent !== THINKING_PLACEHOLDER && !currentRun.firstResponseAt
      ? outputCompletedAt ?? Date.now()
      : currentRun.firstResponseAt;

  return {
    ...messageWithEvents,
    content: nextContent,
    status: getChatMessageStatusFromAgentStatus(output.status),
    agentRun: {
      ...currentRun,
      status: output.status,
      firstResponseAt: finalResponseAt,
      lastResponseAt: finalResponseAt && !currentRun.lastResponseAt ? finalResponseAt : currentRun.lastResponseAt,
      completedAt: outputCompletedAt,
      toolDefinitions: output.toolDefinitions,
      usage: output.usage ?? currentRun.usage,
      finishReason: output.finishReason,
      approvals: getApprovalsForStatus(output.status, currentRun.approvals, output.proposedActions),
      timeline: removeTransientToolTimelineItems(currentRun.timeline),
    },
  };
}

export function applyAgentActionDecisionToChatMessage(
  message: ChatMessage,
  action: AgentProposedAction,
  decision: "approved" | "rejected",
  rejectionMessage?: string,
): ChatMessage {
  const currentRun = ensureAgentRun(message.agentRun, null);
  const actionId = getAgentActionId(action);
  const approvalStatus: AgentApprovalStatus = decision === "approved" ? "approved" : "rejected";
  const rejectedToolResult =
    decision === "rejected" ? createRejectedToolResult(action, rejectionMessage) : null;

  return {
    ...message,
    status: "pending",
    agentRun: {
      ...currentRun,
      status: "running",
      approvals: removeAgentAction(currentRun.approvals, actionId),
      toolCalls: updateToolCallApprovalStatus(currentRun.toolCalls, action, approvalStatus),
      toolResults: rejectedToolResult
        ? upsertById(currentRun.toolResults, rejectedToolResult, (result) => result.callId)
        : currentRun.toolResults,
      diffs: updateDiffApprovalStatus(currentRun.diffs, action, approvalStatus),
      timeline: removeTransientToolTimelineItems(currentRun.timeline),
    },
  };
}

export function applyAgentActionExecutionToChatMessage(
  message: ChatMessage,
  execution: AgentActionExecutionOutput,
): ChatMessage {
  const messageWithAgentOutput = applyAgentOutputToChatMessage(message, execution.agentOutput);
  const currentRun = ensureAgentRun(messageWithAgentOutput.agentRun, execution.agentOutput.runId);

  if (!execution.toolResult) {
    return {
      ...messageWithAgentOutput,
      agentRun: {
        ...currentRun,
        approvals: removeAgentAction(currentRun.approvals, execution.actionId),
        webSearchActivities: normalizeWebSearchActivities(currentRun),
        readActivities: normalizeReadActivities(currentRun),
        timeline: removeTransientToolTimelineItems(currentRun.timeline),
      },
    };
  }

  const finalApprovalStatus: AgentApprovalStatus = execution.status === "rejected" ? "rejected" : "approved";

  return {
    ...messageWithAgentOutput,
    agentRun: {
      ...currentRun,
      toolCalls: currentRun.toolCalls.map((call) =>
        call.id === execution.actionId
          ? {
              ...call,
              approvalStatus: finalApprovalStatus,
            }
          : call,
      ),
      toolResults: upsertById(currentRun.toolResults, execution.toolResult, (result) => result.callId),
      diffs: currentRun.diffs.map((diff) =>
        diff.id === execution.actionId
          ? {
              ...diff,
              approvalStatus: finalApprovalStatus,
            }
          : diff,
      ),
      approvals: removeAgentAction(currentRun.approvals, execution.actionId),
      webSearchActivities: normalizeWebSearchActivities(currentRun),
      readActivities: normalizeReadActivities(currentRun),
      timeline: removeTransientToolTimelineItems(currentRun.timeline),
    },
  };
}
