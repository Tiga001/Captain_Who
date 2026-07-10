// Protocol layer.
export const AGENT_START_RUN_METHOD = 'agent.startRun'
export const AGENT_CANCEL_RUN_METHOD = 'agent.cancelRun'
export const AGENT_START_CONVERSATION_TURN_METHOD = 'agent.startConversationTurn'
export const AGENT_LIST_PENDING_ACTIONS_METHOD = 'agent.listPendingActions'
export const AGENT_APPROVE_ACTION_METHOD = 'agent.approveAction'
export const AGENT_REJECT_ACTION_METHOD = 'agent.rejectAction'
export const AGENT_CANCEL_ACTION_METHOD = 'agent.cancelAction'
export const AGENT_GET_USAGE_SUMMARY_METHOD = 'agent.getUsageSummary'
export const AGENT_CLEAR_USAGE_RECORDS_METHOD = 'agent.clearUsageRecords'
export const AGENT_EVENT_NOTIFICATION_METHOD = 'agent.event'

export type AgentMessageRole = "system" | "user" | "assistant";

export type AgentRunStatus =
  | "idle"
  | "queued"
  | "running"
  | "waiting_for_approval"
  | "completed"
  | "failed"
  | "cancelled"
  | "not_implemented";

export type AgentApiStyle = "openai_compatible" | "anthropic_compatible";

export type AgentSearchMode = "auto" | "disabled" | "tavily";

/**
 * Controls which local paths read-only tools may inspect.
 * - workspace_only: selected workspace and registered attachment paths only.
 * - all: workspace plus absolute local paths and supported aliases such as
 *   @home, @desktop, @documents, and @downloads.
 */
export type AgentReadPermission = "workspace_only" | "all";

/**
 * Controls where file-changing tools may write.
 * - denied: hide/disable file edit tools and reject patch execution.
 * - workspace_only: safe writes inside the selected workspace only.
 * - all: safe writes inside or outside the workspace, including absolute paths and supported aliases.
 */
export type AgentWritePermission = "denied" | "workspace_only" | "all";

/**
 * Controls whether run_command requires a human click.
 * auto_approve only skips the prompt; backend command validation and dangerous-command blocking
 * still apply.
 */
export type AgentCommandPermission = "require_approval" | "auto_approve";

/**
 * Controls whether apply_patch requires a human click.
 * auto_approve only skips the prompt; backend path, symlink, binary, and revision checks still apply.
 */
export type AgentPatchPermission = "require_approval" | "auto_approve";

export interface AgentPermissions {
  read: AgentReadPermission;
  write: AgentWritePermission;
  command: AgentCommandPermission;
  patch: AgentPatchPermission;
}

export type AgentPromptWorkMode = "coding" | "general";

export type AgentPromptTone = "friendly" | "pragmatic";

export type AgentPromptDetailLevel = "low" | "medium" | "high";

export type AgentToolName =
  | "attachments_list"
  | "attachments_list_project"
  | "read_file"
  | "read_image"
  | "read_pdf"
  | "read_word"
  | "read_presentation"
  | "read_spreadsheet"
  | "workspace_map"
  | "search_files"
  | "search_code"
  | "web_search"
  | "web_fetch"
  | "git_diff"
  | "todo_update"
  | "apply_patch"
  | "run_command"
  | (string & {});

export type AgentToolSafety = "read_only" | "requires_approval" | "destructive";

export type AgentApprovalStatus = "not_required" | "required" | "approved" | "rejected";

export type AgentApprovalDecisionStatus = "approved" | "rejected";

export type AgentTodoStatus = "pending" | "in_progress" | "completed" | "blocked";

export type AgentPatchOperation = "create" | "update" | "delete";

export type AgentPatchResultStatus = "applied" | "failed" | "conflict" | "rejected";

export type AgentCommandOutputStream = "stdout" | "stderr";

export type AgentCommandRiskLevel =
  | "read_only"
  | "writes_workspace"
  | "network"
  | "destructive"
  | "unknown";

export interface AgentChatMessage {
  role: AgentMessageRole;
  content: string;
}

export type AgentInputAttachmentKind = "file" | "image";

export type AgentInputAttachmentEncoding = "utf8" | "base64";

export interface AgentInputAttachment {
  id: string;
  kind: AgentInputAttachmentKind;
  name: string;
  mimeType?: string;
  sizeBytes: number;
  encoding: AgentInputAttachmentEncoding;
  data: string;
  truncated?: boolean;
}

export interface AgentConversationMessageAttachment {
  id: string;
  kind: AgentInputAttachmentKind;
  name: string;
  mimeType?: string | null;
  sizeBytes: number;
  previewData?: string | null;
  previewMimeType?: string | null;
  createdAt?: number;
}

export interface AgentWorkspaceContext {
  projectId?: string;
  displayName?: string;
  rootPath?: string;
}

export interface AgentAttachmentReference {
  id: string;
  conversationId: string;
  messageId: string;
  projectId?: string | null;
  kind: AgentInputAttachmentKind;
  name: string;
  mimeType?: string;
  sizeBytes: number;
  readPath: string;
  storageRelPath: string;
  createdAt: number;
}

export interface AgentAttachmentLibraryContext {
  rootPath?: string;
  conversationId?: string;
  projectId?: string | null;
  conversationAttachments: AgentAttachmentReference[];
  projectAttachments: AgentAttachmentReference[];
}

export interface AgentPromptPreferences {
  workMode?: AgentPromptWorkMode;
  tone?: AgentPromptTone;
  detailLevel?: AgentPromptDetailLevel;
  customInstructions?: string;
  updatedAt?: number;
}

export interface AgentRunContext {
  conversationId?: string;
  projectId?: string | null;
  workspace?: AgentWorkspaceContext;
  attachmentLibrary?: AgentAttachmentLibraryContext;
  permissions: AgentPermissions;
}

export interface AgentSearchConfig {
  mode: AgentSearchMode;
  tavilyApiKey?: string;
}

export interface AgentApprovalDecision {
  actionId: string;
  status: AgentApprovalDecisionStatus;
  message?: string;
}

export interface AgentUsage {
  inputTokens?: number;
  outputTokens?: number;
  outputThinkingTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  cacheCreationInputTokens?: number;
  billableRequestCount?: number;
}

export type AgentUsageSummaryRange = "last7Days" | "last30Days" | "all" | "custom";

export interface AgentUsageSummaryInput {
  range: AgentUsageSummaryRange;
  from?: number;
  to?: number;
}

export interface AgentUsageModelSummary {
  modelId: string;
  modelName: string;
  providerPath?: string;
  requestCount: number;
  messageCount: number;
  inputTokens?: number;
  outputTokens?: number;
  outputThinkingTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  cacheCreationInputTokens?: number;
  estimatedCost?: number;
}

export interface AgentUsageSummaryOutput {
  requestCount: number;
  messageCount: number;
  inputTokens?: number;
  outputTokens?: number;
  outputThinkingTokens?: number;
  totalTokens?: number;
  cachedInputTokens?: number;
  cacheCreationInputTokens?: number;
  estimatedCost?: number;
  models: AgentUsageModelSummary[];
}

export interface AgentUsageClearInput {
  from?: number;
  to?: number;
}

export interface AgentUsageClearOutput {
  deletedRecords: number;
}

export interface AgentTodoItem {
  id: string;
  title: string;
  status: AgentTodoStatus;
  note?: string;
  createdAt: number;
  updatedAt: number;
}

export interface AgentTodoState {
  revision: number;
  items: AgentTodoItem[];
  updatedAt: number;
}

export type AgentActionExecutionStatus = "applied" | "approved" | "failed" | "conflict" | "rejected";

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
  agentOutput: AgentChatOutput;
}

export interface AgentChatOutput {
  status: AgentRunStatus;
  content: string;
  runId: string;
  events: AgentEvent[];
  toolDefinitions: AgentToolDefinition[];
  todo?: AgentTodoState;
  usage?: AgentUsage;
  finishReason?: string;
  proposedActions: AgentProposedAction[];
}

export interface AgentConversationTurnInput {
  conversationId?: string;
  projectId?: string | null;
  modelId: string;
  content: string;
  attachments?: AgentInputAttachment[];
  title?: string;
  userMessageId?: string;
  assistantMessageId?: string;
  maxTokens?: number;
  temperature?: number;
  promptPreferences?: AgentPromptPreferences;
  permissions?: AgentPermissions;
}

export interface AgentConversationMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  createdAt: number;
  status?: "pending" | "sent" | "error" | null;
  attachments?: AgentConversationMessageAttachment[];
}

export interface AgentConversationTurnOutput {
  runId: string;
  eventName: string;
  conversationId: string;
  userMessageId: string;
  assistantMessageId: string;
  userMessage: AgentConversationMessage;
  assistantMessage: AgentConversationMessage;
}

export interface AgentStartRunRequest {
  conversationId?: string;
  prompt?: string;
  workspacePath?: string;
}

export interface AgentStartRunResponse {
  runId: string;
  status: AgentRunStatus;
}

export interface AgentCancelRunRequest {
  runId: string;
}

export interface AgentCancelRunResponse {
  runId: string;
  cancelled: boolean;
}

export interface AgentActionIdRequest {
  actionId: string;
}

export interface AgentRejectActionRequest {
  actionId: string;
  message?: string;
}

export type PendingAgentActionStatus =
  | "pending"
  | "approved"
  | "rejected"
  | "cancelled"
  | "completed"
  | "failed";

export interface PendingAgentActionSnapshot {
  actionId: string;
  actionType: string;
  toolName: string;
  toolCallId?: string | null;
  runId: string;
  conversationId?: string | null;
  assistantMessageId?: string | null;
  action: AgentProposedAction;
  createdAt: number;
  status: PendingAgentActionStatus;
}

export interface AgentStateSnapshot {
  status: AgentRunStatus;
  activeRunId: string | null;
  lastError: string | null;
  updatedAt: number;
}

export interface AgentToolCall {
  id: string;
  tool: AgentToolName;
  args: unknown;
  approvalStatus: AgentApprovalStatus;
  reason?: string;
}

export interface AgentToolDefinition {
  name: AgentToolName;
  description: string;
  inputSchema: unknown;
  safety: AgentToolSafety;
  requiresWorkspace: boolean;
  requiresApproval: boolean;
}

export interface AgentToolResult {
  callId: string;
  tool: AgentToolName;
  ok: boolean;
  result?: unknown;
  error?: string;
}

export interface AgentToolContinuation {
  call: AgentToolCall;
  result: AgentToolResult;
}

export interface AgentDiffProposal {
  id: string;
  operation: AgentPatchOperation;
  filePath: string;
  patch: string;
  baseRevision?: string;
  summary?: string;
  approvalStatus: AgentApprovalStatus;
}

export interface AgentPatchResult {
  status: AgentPatchResultStatus;
  operation: AgentPatchOperation;
  filePath: string;
  appliedFilePaths: string[];
  gitDiff?: AgentGitDiffSnapshot;
  gitDiffError?: string;
  error?: string;
  message?: string;
}

export interface AgentGitDiffSnapshot {
  patch: string;
  truncated: boolean;
}

export interface AgentCommandRequest {
  id: string;
  command: string;
  cwd?: string;
  timeoutMs?: number;
  approvalStatus: AgentApprovalStatus;
  riskLevel?: AgentCommandRiskLevel;
  reason?: string;
}

export type AgentProposedAction =
  | { type: "tool_call"; call: AgentToolCall }
  | { type: "diff"; diff: AgentDiffProposal }
  | { type: "command"; command: AgentCommandRequest };

export type AgentEvent =
  | { type: "started"; runId: string; toolDefinitions: AgentToolDefinition[] }
  | { type: "state"; runId: string; state: AgentStateSnapshot }
  | { type: "message_delta"; runId: string; delta: string }
  | { type: "message"; runId: string; content: string }
  | { type: "tool_call"; runId: string; call: AgentToolCall }
  | { type: "tool_result"; runId: string; result: AgentToolResult }
  | { type: "todo_updated"; runId: string; todo: AgentTodoState }
  | { type: "approval_required"; runId: string; action: AgentProposedAction }
  | { type: "diff"; runId: string; diff: AgentDiffProposal }
  | {
      type: "command_output";
      runId: string;
      command: string;
      stream: AgentCommandOutputStream;
      output: string;
    }
  | { type: "error"; runId?: string; message: string; recoverable: boolean }
  | {
      type: "done";
      runId: string;
      success: boolean;
      status?: AgentRunStatus;
      content?: string;
      usage?: AgentUsage;
      finishReason?: string;
      proposedActions?: AgentProposedAction[];
    };
