// Renderer UI.
import type {
  AgentConversationTurnInput,
  AgentConversationTurnOutput,
  AgentPatchResult,
  AgentProposedAction,
  AgentToolResult,
  AgentUsageClearInput,
  AgentUsageClearOutput,
  AgentUsageSummaryInput,
  AgentUsageSummaryOutput,
} from "@mycopilot/protocol";
import { hostClient } from "../../host/hostClient";

export type StartConversationTurnInput = AgentConversationTurnInput;
export type StartConversationTurnOutput = AgentConversationTurnOutput;

export type AgentActionExecutionStatus = "applied" | "approved" | "failed" | "rejected";

export interface AgentCommandExecutionResult {
  command: string;
  cwd: string;
  exitCode?: number;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  cancelled: boolean;
  durationMs: number;
  stdoutTruncated: boolean;
  stderrTruncated: boolean;
  error?: string;
}

export interface AgentActionExecutionOutput {
  actionId: string;
  actionType: string;
  toolName: string;
  status: AgentActionExecutionStatus;
  patchResult?: AgentPatchResult;
  commandResult?: AgentCommandExecutionResult;
  toolResult?: AgentToolResult;
  agentOutput: {
    status: "not_implemented";
    content: string;
    runId: string;
    events: [];
    toolDefinitions: [];
    proposedActions: [];
  };
}

export interface PendingAgentActionSnapshot {
  actionId: string;
  actionType: string;
  toolName: string;
  runId: string;
  conversationId?: string;
  assistantMessageId?: string;
  action: AgentProposedAction;
  createdAt: number;
}

export async function startConversationTurn(
  input: StartConversationTurnInput,
): Promise<StartConversationTurnOutput> {
  const run = await hostClient.agent.startRun({
    conversationId: input.conversationId,
    prompt: input.content,
  });
  const conversationId = input.conversationId ?? `conversation-${Date.now()}`;
  const userMessageId = input.userMessageId ?? `user-${Date.now()}`;
  const assistantMessageId = input.assistantMessageId ?? `assistant-${Date.now()}`;

  return {
    runId: run.runId,
    eventName: "agent_event",
    conversationId,
    userMessageId,
    assistantMessageId,
    userMessage: {
      id: userMessageId,
      role: "user",
      content: input.content,
      createdAt: Date.now(),
      status: "sent",
      attachments: [],
    },
    assistantMessage: {
      id: assistantMessageId,
      role: "assistant",
      content: "Agent runtime is not connected in this UI migration phase.",
      createdAt: Date.now(),
      status: "sent",
      attachments: [],
    },
  };
}

export async function listPendingAgentActions(): Promise<PendingAgentActionSnapshot[]> {
  return [];
}

export async function approveAgentAction(actionId: string): Promise<AgentActionExecutionOutput> {
  return createNotImplementedExecution(actionId, "approved");
}

export async function rejectAgentAction(actionId: string): Promise<AgentActionExecutionOutput> {
  return createNotImplementedExecution(actionId, "rejected");
}

export async function cancelAgentAction(_actionId: string): Promise<boolean> {
  return false;
}

export async function cancelAgentRun(runId: string): Promise<boolean> {
  const response = await hostClient.agent.cancelRun({ runId });
  return response.cancelled;
}

export async function getAgentUsageSummary(
  _input: AgentUsageSummaryInput,
): Promise<AgentUsageSummaryOutput> {
  return {
    requestCount: 0,
    messageCount: 0,
    inputTokens: 0,
    outputTokens: 0,
    totalTokens: 0,
    cachedInputTokens: 0,
    cacheCreationInputTokens: 0,
    estimatedCost: 0,
    models: [],
  };
}

export async function clearAgentUsageRecords(
  _input: AgentUsageClearInput = {},
): Promise<AgentUsageClearOutput> {
  return { deletedRecords: 0 };
}

function createNotImplementedExecution(
  actionId: string,
  status: AgentActionExecutionStatus,
): AgentActionExecutionOutput {
  return {
    actionId,
    actionType: "stub",
    toolName: "stub",
    status,
    agentOutput: {
      status: "not_implemented",
      content: "",
      runId: "stub",
      events: [],
      toolDefinitions: [],
      proposedActions: [],
    },
  };
}
