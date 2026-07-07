// Renderer UI.
import type {
  AgentDiffProposal,
  AgentInputAttachment,
  AgentProposedAction,
  AgentRunStatus,
  AgentStateSnapshot,
  AgentToolCall,
  AgentToolDefinition,
  AgentToolResult,
  AgentUsage,
} from "@mycopilot/protocol";

export interface ChatWebSearchSource {
  id: string;
  title: string;
  url: string;
  displayUrl: string;
  domain: string;
  faviconDataUrl?: string;
  faviconMimeType?: string;
  snippet?: string;
  score?: number;
  publishedDate?: string;
}

export interface ChatWebSearchActivity {
  callId: string;
  kind?: "search" | "fetch";
  query: string;
  provider: string;
  status: "running" | "completed" | "failed" | "cancelled";
  sources: ChatWebSearchSource[];
  answer?: string;
  error?: string;
  responseTime?: number | string | null;
  truncated?: boolean;
  updatedAt: number;
}

export type ChatReadActivityKind =
  | "file"
  | "image"
  | "pdf"
  | "word"
  | "presentation"
  | "spreadsheet";

export interface ChatReadActivity {
  callId: string;
  tool: AgentToolCall["tool"];
  kind: ChatReadActivityKind;
  status: "running" | "completed" | "failed" | "cancelled";
  path: string;
  fileName: string;
  extension?: string;
  mimeType?: string;
  thumbnailDataUrl?: string;
  error?: string;
  updatedAt: number;
}

export type ChatAgentTimelineItem =
  | { id: string; type: "message"; content: string }
  | { id: string; type: "tool_call"; callId: string }
  | { id: string; type: "error"; message: string };

export interface ChatAgentRunView {
  runId: string | null;
  status: AgentRunStatus | "starting";
  startedAt?: number;
  firstResponseAt?: number;
  lastResponseAt?: number;
  completedAt?: number;
  toolDefinitions: AgentToolDefinition[];
  toolCalls: AgentToolCall[];
  toolResults: AgentToolResult[];
  webSearchActivities?: ChatWebSearchActivity[];
  readActivities?: ChatReadActivity[];
  approvals: AgentProposedAction[];
  diffs: AgentDiffProposal[];
  timeline: ChatAgentTimelineItem[];
  state?: AgentStateSnapshot;
  error?: string;
  usage?: AgentUsage;
  finishReason?: string;
}

export interface ChatMessageUiState {
  timelineCollapsed?: boolean;
}

export interface ChatMessageAttachment {
  id: string;
  kind: "file" | "image";
  name: string;
  mimeType?: string | null;
  sizeBytes: number;
  encoding?: AgentInputAttachment["encoding"];
  data?: string;
  previewData?: string | null;
  previewMimeType?: string | null;
  createdAt?: number;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  createdAt: number;
  status?: "pending" | "sent" | "error";
  attachments?: ChatMessageAttachment[];
  agentRun?: ChatAgentRunView;
  uiState?: ChatMessageUiState;
}

export type ChatPermissionMode = "default" | "full" | "custom";

export interface ChatComposerDraft {
  message: string;
  permissionMode: ChatPermissionMode;
  modelId: string;
  projectId: string | null;
  attachments: AgentInputAttachment[];
  updatedAt: number;
}

export interface ChatSubmitOptions {
  modelId: string;
  permissionMode: ChatPermissionMode;
  projectId: string | null;
  attachments?: AgentInputAttachment[];
}

export interface ChatConversation {
  id: string;
  projectId: string | null;
  modelId: string | null;
  title: string;
  messages: ChatMessage[];
  createdAt: number;
  updatedAt: number;
  pinnedAt?: number | null;
  archivedAt?: number | null;
  unreadAt?: number | null;
}
