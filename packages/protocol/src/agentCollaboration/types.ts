import type {
  AGENT_COLLABORATION_SCHEMA_VERSION,
  AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION,
  AGENT_COLLABORATION_EVENT_SCHEMA_VERSION
} from './constants'
import type { HumanInteractionResponseDisplay } from '../humanInteraction'
import type { AgentEvent, AgentProposedAction } from '../agent'

export interface AgentCollaborationSettings {
  enabled: boolean
  revision: number
  updatedAt: number
}

export type AgentCollaborationSettingsGetInput = Record<string, never>

export interface AgentCollaborationSettingsUpdate {
  enabled: boolean
  expectedRevision: number
}

export type AgentLifecycleView = 'active' | 'archived' | 'disabled'

export type AgentDisplayStatusView =
  | 'idle'
  | 'queued'
  | 'running'
  | 'waiting_approval'
  | 'latest_completed'
  | 'latest_failed'
  | 'latest_interrupted'
  | 'latest_outcome_unknown'
  | 'archived'
  | 'disabled'

export interface AgentModelDisplay {
  modelConfigId: string
  displayName: string
}

export interface AgentSummary {
  agentId: string
  rootAgentId: string
  rootConversationId: string
  parentAgentId: string | null
  conversationId: string
  projectId: string | null
  taskName: string
  taskPath: string
  lifecycle: AgentLifecycleView
  displayStatus: AgentDisplayStatusView
  latestActivityAt: number
  model: AgentModelDisplay | null
}

export interface AgentTreeRequest {
  rootConversationId: string
}

export interface AgentTreeSnapshot {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  workspaceId: string | null
  projectId: string | null
  rootAgentId: string
  rootConversationId: string
  agents: AgentSummary[]
  lastSequence: number
}

export interface AgentTreeLookup {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  materialized: boolean
  tree: AgentTreeSnapshot | null
}

export interface AgentDetailRequest extends AgentTreeRequest {
  agentId: string
}

export interface AgentTemplateBinding {
  templateId: string
  machineKey: string
  name: string
  description: string
  revision: number
}

export interface AgentDetail {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  summary: AgentSummary
  template: AgentTemplateBinding | null
  reasoningEffort: string | null
  revision: number
  createdAt: number
  updatedAt: number
}

export interface AgentConversationLocatorRequest extends AgentDetailRequest {}

export interface AgentConversationLocator {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  agentId: string
  conversationId: string
  mode: 'interactive' | 'observer'
}

export interface AgentObserverConversationRequest {
  rootConversationId: string
  conversationId: string
}

export interface AgentObserverInputOrigin {
  kind: 'human' | 'agent' | 'historical_snapshot'
  senderAgentId: string | null
  sourceAgentMessageId: string | null
  snapshotSourceConversationId: string | null
  snapshotSourceMessageId: string | null
}

export interface AgentObserverMessage {
  readonly humanInteractionResponse?: HumanInteractionResponseDisplay | null
  messageId: string
  role: string
  content: string
  createdAt: number
  status: string | null
  inputOrigin: AgentObserverInputOrigin | null
  attachments: AgentObserverAttachment[]
  agentRunJson: string | null
  uiStateJson: string | null
}

export interface AgentObserverAttachment {
  attachmentId: string
  kind: string
  name: string
  mimeType: string | null
  sizeBytes: number
  previewData: string | null
  previewMimeType: string | null
  createdAt: number
}

export interface AgentObserverConversation {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  agentId: string
  rootConversationId: string
  conversationId: string
  projectId: string | null
  modelId: string | null
  title: string
  createdAt: number
  updatedAt: number
  messages: AgentObserverMessage[]
  /** Process-local text at the same read cut as the persisted conversation. */
  liveStream?: AgentObserverLiveStreamSnapshot
}

export interface AgentObserverStreamCursor {
  generation: string
  sequence: number
}

export interface AgentObserverLiveStreamSnapshot {
  runId: string
  assistantMessageId: string
  cursor: AgentObserverStreamCursor
  stream: {
    streamId: string
    attempt: number
    content: string
    traceBoundarySequence: number
    committed: boolean
  } | null
}

export type CollaborationEventKind =
  | 'agent_created'
  | 'agent_updated'
  | 'mailbox_enqueued'
  | 'mailbox_updated'
  | 'wake_created'
  | 'wake_updated'
  | 'turn_started'
  | 'turn_updated'
  | 'approval_projected'
  | 'approval_updated'

export type CollaborationActivitySemantic =
  'started' | 'updated' | 'waiting_approval' | 'completed' | 'failed' | 'interrupted'

export interface CollaborationActivitySnapshot {
  schemaVersion: typeof AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION
  activityId: string
  semantic: CollaborationActivitySemantic
  /** Presentation subject; the outer event agent remains the invalidation subject. */
  agentId: string
  taskNameSnapshot: string
  /** The actual task dispatcher, or recipient of an ordinary message, owns this projection. */
  ownerAgentId: string
  ownerConversationId: string
  /** Immutable Task/Followup mailbox identity; ordinary Message updates use null. */
  taskMessageId: string | null
  /** Owner message containing or preceding the activity; null means before its first message. */
  anchorMessageId: string | null
  /** Insert inside the active owner trace at this boundary; null means after the anchor message. */
  traceBoundarySequence: number | null
}

/** Host-confirmed transfer metadata; message content stays in the existing delivery channel. */
export interface CollaborationTransmission {
  /** Stable transfer identity, independent of the enclosing event identity. */
  id: string
  kind: 'message' | 'task' | 'user_message' | 'completion'
  /** Null denotes the human user, only for user_message or completion. */
  sourceAgentId: string | null
  targetAgentId: string | null
}

export interface CollaborationEventEnvelope {
  schemaVersion: typeof AGENT_COLLABORATION_EVENT_SCHEMA_VERSION
  eventId: string
  sequence: number
  workspaceId: string | null
  projectId: string | null
  rootAgentId: string
  rootConversationId: string
  agentId: string
  conversationId: string
  turnId: string | null
  runId: string | null
  messageId: string | null
  kind: CollaborationEventKind
  resourceRevision: number
  activities: CollaborationActivitySnapshot[]
  transmission?: CollaborationTransmission
  occurredAt: number
}

/**
 * Process-local, presentation-only stream for an authorized child Conversation observer.
 *
 * Durable collaboration events and observer snapshots remain the recovery source of truth. This
 * envelope adds the exact Host-owned identities which the legacy `agent.event` stream does not
 * carry, so two child Conversations can safely reuse the existing Renderer Agent-event reducer.
 */
export interface AgentObserverEventEnvelope {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  rootAgentId: string
  rootConversationId: string
  agentId: string
  conversationId: string
  runId: string
  assistantMessageId: string
  event: AgentEvent
  streamCursor?: AgentObserverStreamCursor
}

export interface CollaborationEventsRequest extends AgentTreeRequest {
  afterSequence: number
  limit: number
}

export interface CollaborationEventsPage {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  rootAgentId: string
  rootConversationId: string
  events: CollaborationEventEnvelope[]
  lastSequence: number
  hasMore: boolean
}

export interface CollaborationResyncEnvelope {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  reason: 'core_started' | 'model_settings_changed'
}

export interface AgentTemplate {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  templateId: string
  projectIds: string[]
  machineKey: string
  name: string
  description: string
  instructions: string
  modelConfigId: string
  modelDisplayName: string | null
  enabled: boolean
  revision: number
  createdAt: number
  updatedAt: number
}

export interface AgentTemplateListRequest {
  includeDisabled: boolean
}

export interface AgentTemplateList {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  templates: AgentTemplate[]
}

export interface AgentTemplateCreateRequest {
  templateId: string
  machineKey: string
  name: string
  description: string
  instructions: string
  modelConfigId: string
  enabled: boolean
}

export interface AgentTemplateUpdateRequest {
  templateId: string
  expectedRevision: number
  name: string
  description: string
  instructions: string
  modelConfigId: string
}

export interface AgentTemplateSetEnabledRequest {
  templateId: string
  expectedRevision: number
  enabled: boolean
}

export interface AgentTemplateDeleteRequest {
  templateId: string
  expectedRevision: number
}

export interface AgentTemplateProjectAssignmentRequest {
  projectId: string
  templateId: string
  assigned: boolean
}

export type CollaborationApprovalStatus =
  | 'pending'
  | 'approved'
  | 'executing'
  | 'rejected'
  | 'cancelled'
  | 'completed'
  | 'failed'
  | 'expired'
  | 'interrupted'

export interface CollaborationApprovalProjection {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  approvalId: string
  rootAgentId: string
  rootConversationId: string
  sourceAgentId: string
  sourceTaskPath: string
  sourceConversationId: string
  runId: string
  actionId: string
  actionType: string
  toolName: string
  action: AgentProposedAction
  status: CollaborationApprovalStatus
  createdAt: number
  updatedAt: number
}

export interface CollaborationApprovalListRequest extends AgentTreeRequest {}

export interface CollaborationApprovalList {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  approvals: CollaborationApprovalProjection[]
}

export interface CollaborationApprovalDecisionRequest extends AgentTreeRequest {
  approvalId: string
  decision: 'approve' | 'reject' | 'cancel'
  message: string | null
}

export interface CollaborationApprovalDecisionResult {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  approvalId: string
  accepted: boolean
  alreadySettled: boolean
  status: CollaborationApprovalStatus
}
