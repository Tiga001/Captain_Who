import type {
  AgentEvent,
  AgentFileDraftSnapshot,
  AgentProposedAction,
  AgentStateSnapshot,
  AgentToolCall,
  AgentToolResult,
  AgentUsage
} from './agent'
import type { ActivatedSkillSummary } from './skills'
import {
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationProposedAction,
  parseAgentEventForHost,
  parseAgentToolIdentityForHost,
  parseAgentMcpToolInvocationEvent,
  parsePendingAgentActionSnapshotsForHost
} from './agentMcpParsers'
import {
  isAgentCommandSessionEventType,
  parseAgentCommandSessionEvent
} from './agentCommandSessionParsers'

export const AGENT_COLLABORATION_SCHEMA_VERSION = 1 as const
export const AGENT_COLLABORATION_EVENT_SCHEMA_VERSION = 2 as const
export const AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION = 2 as const

export const AGENT_COLLABORATION_GET_TREE_METHOD = 'agent.collaboration.getTree'
export const AGENT_COLLABORATION_GET_AGENT_METHOD = 'agent.collaboration.getAgent'
export const AGENT_COLLABORATION_LOCATE_CONVERSATION_METHOD =
  'agent.collaboration.locateConversation'
export const AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD =
  'agent.collaboration.loadObserverConversation'
export const AGENT_COLLABORATION_LIST_EVENTS_METHOD = 'agent.collaboration.listEvents'
export const AGENT_COLLABORATION_TEMPLATES_LIST_METHOD = 'agent.collaboration.templates.list'
export const AGENT_COLLABORATION_TEMPLATES_CREATE_METHOD = 'agent.collaboration.templates.create'
export const AGENT_COLLABORATION_TEMPLATES_UPDATE_METHOD = 'agent.collaboration.templates.update'
export const AGENT_COLLABORATION_TEMPLATES_SET_ENABLED_METHOD =
  'agent.collaboration.templates.setEnabled'
export const AGENT_COLLABORATION_TEMPLATES_DELETE_METHOD = 'agent.collaboration.templates.delete'
export const AGENT_COLLABORATION_APPROVALS_LIST_METHOD = 'agent.collaboration.approvals.list'
export const AGENT_COLLABORATION_APPROVALS_DECIDE_METHOD = 'agent.collaboration.approvals.decide'
export const AGENT_COLLABORATION_EVENT_NOTIFICATION_METHOD = 'agent.collaboration.event'
export const AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD =
  'agent.collaboration.observerEvent'
export const AGENT_COLLABORATION_RESYNC_NOTIFICATION_METHOD = 'agent.collaboration.resync'

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
  semantic: CollaborationActivitySemantic
  /** Presentation subject; the outer event agent remains the invalidation subject. */
  agentId: string
  taskNameSnapshot: string
  /** Trusted assistant message in the root Conversation, or null when no such identity exists. */
  rootAnchorMessageId: string | null
  /** Insert before the first backend-owned root Timeline item at or after this trace sequence. */
  rootTraceBoundarySequence: number | null
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
  activity: CollaborationActivitySnapshot | null
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
  projectId: string
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
  projectId: string
  includeDisabled: boolean
}

export interface AgentTemplateList {
  schemaVersion: typeof AGENT_COLLABORATION_SCHEMA_VERSION
  templates: AgentTemplate[]
}

export interface AgentTemplateCreateRequest {
  templateId: string
  projectId: string
  machineKey: string
  name: string
  description: string
  instructions: string
  modelConfigId: string
  enabled: boolean
}

export interface AgentTemplateUpdateRequest {
  projectId: string
  templateId: string
  expectedRevision: number
  name: string
  description: string
  instructions: string
  modelConfigId: string
}

export interface AgentTemplateSetEnabledRequest {
  projectId: string
  templateId: string
  expectedRevision: number
  enabled: boolean
}

export interface AgentTemplateDeleteRequest {
  projectId: string
  templateId: string
  expectedRevision: number
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

const MAX_ID_BYTES = 512
const MAX_TEXT_BYTES = 65_536
const MAX_EVENT_PAGE = 512
const MAX_OBSERVER_PREVIEW_BYTES = 48 * 1024 * 1024
const MAX_OBSERVER_STATE_JSON_BYTES = 32 * 1024 * 1024

type JsonRecord = Record<string, unknown>

function record(value: unknown, context: string): JsonRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as JsonRecord
}

function exact(value: JsonRecord, keys: readonly string[], context: string): void {
  const allowed = new Set(keys)
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw new Error(`Invalid ${context}.${key}`)
  }
  for (const key of keys) {
    if (!(key in value)) throw new Error(`Missing ${context}.${key}`)
  }
}

function text(value: unknown, context: string, maximum = MAX_ID_BYTES): string {
  if (typeof value !== 'string' || value.length === 0 || value.trim() !== value) {
    throw new Error(`Invalid ${context}`)
  }
  if (new TextEncoder().encode(value).byteLength > maximum) throw new Error(`Invalid ${context}`)
  return value
}

function nullableText(value: unknown, context: string): string | null {
  return value === null ? null : text(value, context)
}

function boundedOptionalString(
  value: unknown,
  context: string,
  maximum: number,
  requireJson = false
): string | null {
  if (value === null) return null
  if (typeof value !== 'string' || new TextEncoder().encode(value).byteLength > maximum) {
    throw new Error(`Invalid ${context}`)
  }
  if (requireJson) {
    try {
      JSON.parse(value)
    } catch {
      throw new Error(`Invalid ${context}`)
    }
  }
  return value
}

function integer(value: unknown, context: string, minimum = 0, maximum = Number.MAX_SAFE_INTEGER) {
  if (!Number.isSafeInteger(value) || (value as number) < minimum || (value as number) > maximum) {
    throw new Error(`Invalid ${context}`)
  }
  return value as number
}

function bool(value: unknown, context: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`Invalid ${context}`)
  return value
}

function oneOf<const T extends readonly string[]>(
  value: unknown,
  choices: T,
  context: string
): T[number] {
  if (typeof value !== 'string' || !(choices as readonly string[]).includes(value)) {
    throw new Error(`Invalid ${context}`)
  }
  return value as T[number]
}

function schema(value: unknown, context: string): typeof AGENT_COLLABORATION_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_SCHEMA_VERSION
}

function eventSchema(
  value: unknown,
  context: string
): typeof AGENT_COLLABORATION_EVENT_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_EVENT_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_EVENT_SCHEMA_VERSION
}

function activitySchema(
  value: unknown,
  context: string
): typeof AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
  if (value !== AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION)
    throw new Error(`Invalid ${context}.schemaVersion`)
  return AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION
}

export function parseAgentTreeRequest(value: unknown): AgentTreeRequest {
  const item = record(value, 'AgentTreeRequest')
  exact(item, ['rootConversationId'], 'AgentTreeRequest')
  return { rootConversationId: text(item.rootConversationId, 'rootConversationId') }
}

export function parseAgentDetailRequest(value: unknown): AgentDetailRequest {
  const item = record(value, 'AgentDetailRequest')
  exact(item, ['rootConversationId', 'agentId'], 'AgentDetailRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId')
  }
}

export const parseAgentConversationLocatorRequest = parseAgentDetailRequest

function parseModel(value: unknown, context: string): AgentModelDisplay | null {
  if (value === null) return null
  const item = record(value, context)
  exact(item, ['modelConfigId', 'displayName'], context)
  return {
    modelConfigId: text(item.modelConfigId, `${context}.modelConfigId`),
    displayName: text(item.displayName, `${context}.displayName`)
  }
}

export function parseAgentSummary(value: unknown, context = 'AgentSummary'): AgentSummary {
  const item = record(value, context)
  exact(
    item,
    [
      'agentId',
      'rootAgentId',
      'rootConversationId',
      'parentAgentId',
      'conversationId',
      'projectId',
      'taskName',
      'taskPath',
      'lifecycle',
      'displayStatus',
      'latestActivityAt',
      'model'
    ],
    context
  )
  return {
    agentId: text(item.agentId, `${context}.agentId`),
    rootAgentId: text(item.rootAgentId, `${context}.rootAgentId`),
    rootConversationId: text(item.rootConversationId, `${context}.rootConversationId`),
    parentAgentId: nullableText(item.parentAgentId, `${context}.parentAgentId`),
    conversationId: text(item.conversationId, `${context}.conversationId`),
    projectId: nullableText(item.projectId, `${context}.projectId`),
    taskName: text(item.taskName, `${context}.taskName`),
    taskPath: text(item.taskPath, `${context}.taskPath`, 4_096),
    lifecycle: oneOf(
      item.lifecycle,
      ['active', 'archived', 'disabled'] as const,
      `${context}.lifecycle`
    ),
    displayStatus: oneOf(
      item.displayStatus,
      [
        'idle',
        'queued',
        'running',
        'waiting_approval',
        'latest_completed',
        'latest_failed',
        'latest_interrupted',
        'latest_outcome_unknown',
        'archived',
        'disabled'
      ] as const,
      `${context}.displayStatus`
    ),
    latestActivityAt: integer(item.latestActivityAt, `${context}.latestActivityAt`),
    model: parseModel(item.model, `${context}.model`)
  }
}

export function parseAgentTreeSnapshot(value: unknown): AgentTreeSnapshot {
  const item = record(value, 'AgentTreeSnapshot')
  exact(
    item,
    [
      'schemaVersion',
      'workspaceId',
      'projectId',
      'rootAgentId',
      'rootConversationId',
      'agents',
      'lastSequence'
    ],
    'AgentTreeSnapshot'
  )
  if (!Array.isArray(item.agents) || item.agents.length > 1_024)
    throw new Error('Invalid AgentTreeSnapshot.agents')
  const parsed: AgentTreeSnapshot = {
    schemaVersion: schema(item.schemaVersion, 'AgentTreeSnapshot'),
    workspaceId: nullableText(item.workspaceId, 'workspaceId'),
    projectId: nullableText(item.projectId, 'projectId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agents: item.agents.map((entry, index) => parseAgentSummary(entry, `agents[${index}]`)),
    lastSequence: integer(item.lastSequence, 'lastSequence')
  }
  const agentsById = new Map<string, AgentSummary>()
  const conversationIds = new Set<string>()
  const taskPaths = new Set<string>()
  for (const agent of parsed.agents) {
    if (
      agentsById.has(agent.agentId) ||
      conversationIds.has(agent.conversationId) ||
      taskPaths.has(agent.taskPath)
    ) {
      throw new Error('Invalid AgentTreeSnapshot duplicate identity')
    }
    agentsById.set(agent.agentId, agent)
    conversationIds.add(agent.conversationId)
    taskPaths.add(agent.taskPath)
  }
  const root = agentsById.get(parsed.rootAgentId)
  if (
    parsed.workspaceId !== parsed.projectId ||
    root === undefined ||
    root.parentAgentId !== null ||
    root.conversationId !== parsed.rootConversationId ||
    parsed.agents.some(
      (agent) =>
        agent.rootAgentId !== parsed.rootAgentId ||
        agent.rootConversationId !== parsed.rootConversationId ||
        agent.projectId !== parsed.projectId
    )
  ) {
    throw new Error('Invalid AgentTreeSnapshot identity')
  }
  for (const agent of parsed.agents) {
    const visited = new Set<string>()
    let current: AgentSummary | undefined = agent
    while (current.agentId !== parsed.rootAgentId) {
      if (visited.has(current.agentId) || current.parentAgentId === null) {
        throw new Error('Invalid AgentTreeSnapshot parent relation')
      }
      visited.add(current.agentId)
      current = agentsById.get(current.parentAgentId)
      if (current === undefined) throw new Error('Invalid AgentTreeSnapshot parent relation')
    }
  }
  return parsed
}

export function parseAgentTreeLookup(value: unknown): AgentTreeLookup {
  const item = record(value, 'AgentTreeLookup')
  exact(item, ['schemaVersion', 'materialized', 'tree'], 'AgentTreeLookup')
  const materialized = bool(item.materialized, 'materialized')
  const tree = item.tree === null ? null : parseAgentTreeSnapshot(item.tree)
  if (materialized !== (tree !== null)) throw new Error('Invalid AgentTreeLookup state')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTreeLookup'),
    materialized,
    tree
  }
}

export function parseAgentDetail(value: unknown): AgentDetail {
  const item = record(value, 'AgentDetail')
  exact(
    item,
    [
      'schemaVersion',
      'summary',
      'template',
      'reasoningEffort',
      'revision',
      'createdAt',
      'updatedAt'
    ],
    'AgentDetail'
  )
  let template: AgentTemplateBinding | null = null
  if (item.template !== null) {
    const binding = record(item.template, 'AgentDetail.template')
    exact(
      binding,
      ['templateId', 'machineKey', 'name', 'description', 'revision'],
      'AgentDetail.template'
    )
    template = {
      templateId: text(binding.templateId, 'templateId'),
      machineKey: text(binding.machineKey, 'machineKey'),
      name: text(binding.name, 'name'),
      description:
        typeof binding.description === 'string'
          ? binding.description
          : (() => {
              throw new Error('Invalid description')
            })(),
      revision: integer(binding.revision, 'revision', 1)
    }
  }
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentDetail'),
    summary: parseAgentSummary(item.summary),
    template,
    reasoningEffort: nullableText(item.reasoningEffort, 'reasoningEffort'),
    revision: integer(item.revision, 'revision', 1),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseAgentConversationLocator(value: unknown): AgentConversationLocator {
  const item = record(value, 'AgentConversationLocator')
  exact(item, ['schemaVersion', 'agentId', 'conversationId', 'mode'], 'AgentConversationLocator')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentConversationLocator'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    mode: oneOf(item.mode, ['interactive', 'observer'] as const, 'mode')
  }
}

export function parseAgentObserverConversationRequest(
  value: unknown
): AgentObserverConversationRequest {
  const item = record(value, 'AgentObserverConversationRequest')
  exact(item, ['rootConversationId', 'conversationId'], 'AgentObserverConversationRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    conversationId: text(item.conversationId, 'conversationId')
  }
}

function parseObserverOrigin(value: unknown, context: string): AgentObserverInputOrigin | null {
  if (value === null) return null
  const item = record(value, context)
  exact(
    item,
    [
      'kind',
      'senderAgentId',
      'sourceAgentMessageId',
      'snapshotSourceConversationId',
      'snapshotSourceMessageId'
    ],
    context
  )
  const parsed = {
    kind: oneOf(item.kind, ['human', 'agent', 'historical_snapshot'] as const, `${context}.kind`),
    senderAgentId: nullableText(item.senderAgentId, `${context}.senderAgentId`),
    sourceAgentMessageId: nullableText(
      item.sourceAgentMessageId,
      `${context}.sourceAgentMessageId`
    ),
    snapshotSourceConversationId: nullableText(
      item.snapshotSourceConversationId,
      `${context}.snapshotSourceConversationId`
    ),
    snapshotSourceMessageId: nullableText(
      item.snapshotSourceMessageId,
      `${context}.snapshotSourceMessageId`
    )
  }
  const hasAgent = parsed.senderAgentId !== null && parsed.sourceAgentMessageId !== null
  const hasNoAgent = parsed.senderAgentId === null && parsed.sourceAgentMessageId === null
  const hasSnapshot =
    parsed.snapshotSourceConversationId !== null && parsed.snapshotSourceMessageId !== null
  const hasNoSnapshot =
    parsed.snapshotSourceConversationId === null && parsed.snapshotSourceMessageId === null
  if (
    (parsed.kind === 'human' && (!hasNoAgent || !hasNoSnapshot)) ||
    (parsed.kind === 'agent' && (!hasAgent || !hasNoSnapshot)) ||
    (parsed.kind === 'historical_snapshot' && (!hasSnapshot || (!hasAgent && !hasNoAgent)))
  ) {
    throw new Error(`Invalid ${context} identity`)
  }
  return parsed
}

export function parseAgentObserverConversation(value: unknown): AgentObserverConversation | null {
  if (value === null) return null
  const item = record(value, 'AgentObserverConversation')
  exact(
    item,
    [
      'schemaVersion',
      'agentId',
      'rootConversationId',
      'conversationId',
      'projectId',
      'modelId',
      'title',
      'createdAt',
      'updatedAt',
      'messages'
    ],
    'AgentObserverConversation'
  )
  if (!Array.isArray(item.messages) || item.messages.length > 100_000) {
    throw new Error('Invalid AgentObserverConversation.messages')
  }
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentObserverConversation'),
    agentId: text(item.agentId, 'agentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    conversationId: text(item.conversationId, 'conversationId'),
    projectId: nullableText(item.projectId, 'projectId'),
    modelId: nullableText(item.modelId, 'modelId'),
    title:
      typeof item.title === 'string'
        ? item.title
        : (() => {
            throw new Error('Invalid title')
          })(),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt'),
    messages: item.messages.map((value, index) => {
      const context = `messages[${index}]`
      const message = record(value, context)
      exact(
        message,
        [
          'messageId',
          'role',
          'content',
          'createdAt',
          'status',
          'inputOrigin',
          'attachments',
          'agentRunJson',
          'uiStateJson'
        ],
        context
      )
      if (typeof message.content !== 'string') throw new Error(`Invalid ${context}.content`)
      const role = oneOf(message.role, ['user', 'assistant'] as const, `${context}.role`)
      const status =
        message.status === null
          ? null
          : oneOf(message.status, ['pending', 'sent', 'error'] as const, `${context}.status`)
      const inputOrigin = parseObserverOrigin(message.inputOrigin, `${context}.inputOrigin`)
      if (
        (role === 'user' && inputOrigin === null) ||
        (role === 'assistant' && inputOrigin !== null)
      ) {
        throw new Error(`Invalid ${context} origin`)
      }
      if (!Array.isArray(message.attachments) || message.attachments.length > 256) {
        throw new Error(`Invalid ${context}.attachments`)
      }
      return {
        messageId: text(message.messageId, `${context}.messageId`),
        role,
        content: message.content,
        createdAt: integer(message.createdAt, `${context}.createdAt`),
        status,
        inputOrigin,
        attachments: message.attachments.map((value, attachmentIndex) => {
          const attachmentContext = `${context}.attachments[${attachmentIndex}]`
          const attachment = record(value, attachmentContext)
          exact(
            attachment,
            [
              'attachmentId',
              'kind',
              'name',
              'mimeType',
              'sizeBytes',
              'previewData',
              'previewMimeType',
              'createdAt'
            ],
            attachmentContext
          )
          return {
            attachmentId: text(attachment.attachmentId, `${attachmentContext}.attachmentId`),
            kind: text(attachment.kind, `${attachmentContext}.kind`),
            name: text(attachment.name, `${attachmentContext}.name`, 4_096),
            mimeType: nullableText(attachment.mimeType, `${attachmentContext}.mimeType`),
            sizeBytes: integer(attachment.sizeBytes, `${attachmentContext}.sizeBytes`),
            previewData: boundedOptionalString(
              attachment.previewData,
              `${attachmentContext}.previewData`,
              MAX_OBSERVER_PREVIEW_BYTES
            ),
            previewMimeType: nullableText(
              attachment.previewMimeType,
              `${attachmentContext}.previewMimeType`
            ),
            createdAt: integer(attachment.createdAt, `${attachmentContext}.createdAt`)
          }
        }),
        agentRunJson: boundedOptionalString(
          message.agentRunJson,
          `${context}.agentRunJson`,
          MAX_OBSERVER_STATE_JSON_BYTES,
          true
        ),
        uiStateJson: boundedOptionalString(
          message.uiStateJson,
          `${context}.uiStateJson`,
          MAX_OBSERVER_STATE_JSON_BYTES,
          true
        )
      }
    })
  }
}

export function parseCollaborationEventEnvelope(value: unknown): CollaborationEventEnvelope {
  const item = record(value, 'CollaborationEventEnvelope')
  exact(
    item,
    [
      'schemaVersion',
      'eventId',
      'sequence',
      'workspaceId',
      'projectId',
      'rootAgentId',
      'rootConversationId',
      'agentId',
      'conversationId',
      'turnId',
      'runId',
      'messageId',
      'kind',
      'resourceRevision',
      'activity',
      'occurredAt'
    ],
    'CollaborationEventEnvelope'
  )
  const activity =
    item.activity === null
      ? null
      : (() => {
          const snapshot = record(item.activity, 'CollaborationActivitySnapshot')
          exact(
            snapshot,
            [
              'schemaVersion',
              'semantic',
              'agentId',
              'taskNameSnapshot',
              'rootAnchorMessageId',
              'rootTraceBoundarySequence'
            ],
            'CollaborationActivitySnapshot'
          )
          return {
            schemaVersion: activitySchema(snapshot.schemaVersion, 'CollaborationActivitySnapshot'),
            semantic: oneOf(
              snapshot.semantic,
              [
                'started',
                'updated',
                'waiting_approval',
                'completed',
                'failed',
                'interrupted'
              ] as const,
              'CollaborationActivitySnapshot.semantic'
            ),
            agentId: text(snapshot.agentId, 'CollaborationActivitySnapshot.agentId'),
            taskNameSnapshot: text(
              snapshot.taskNameSnapshot,
              'CollaborationActivitySnapshot.taskNameSnapshot',
              256
            ),
            rootAnchorMessageId: nullableText(
              snapshot.rootAnchorMessageId,
              'CollaborationActivitySnapshot.rootAnchorMessageId'
            ),
            rootTraceBoundarySequence:
              snapshot.rootTraceBoundarySequence === null
                ? null
                : integer(
                    snapshot.rootTraceBoundarySequence,
                    'CollaborationActivitySnapshot.rootTraceBoundarySequence',
                    0
                  )
          } satisfies CollaborationActivitySnapshot
        })()
  if (
    activity !== null &&
    (activity.rootAnchorMessageId === null) !== (activity.rootTraceBoundarySequence === null)
  ) {
    throw new Error(
      'Invalid CollaborationActivitySnapshot: root placement fields must be both present or both null'
    )
  }
  const parsed: CollaborationEventEnvelope = {
    schemaVersion: eventSchema(item.schemaVersion, 'CollaborationEventEnvelope'),
    eventId: text(item.eventId, 'eventId'),
    sequence: integer(item.sequence, 'sequence', 1),
    workspaceId: nullableText(item.workspaceId, 'workspaceId'),
    projectId: nullableText(item.projectId, 'projectId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    turnId: nullableText(item.turnId, 'turnId'),
    runId: nullableText(item.runId, 'runId'),
    messageId: nullableText(item.messageId, 'messageId'),
    kind: oneOf(
      item.kind,
      [
        'agent_created',
        'agent_updated',
        'mailbox_enqueued',
        'mailbox_updated',
        'wake_created',
        'wake_updated',
        'turn_started',
        'turn_updated',
        'approval_projected',
        'approval_updated'
      ] as const,
      'kind'
    ),
    resourceRevision: integer(item.resourceRevision, 'resourceRevision', 1),
    activity,
    occurredAt: integer(item.occurredAt, 'occurredAt')
  }
  if (parsed.workspaceId !== parsed.projectId) {
    throw new Error('Invalid CollaborationEventEnvelope identity')
  }
  if (parsed.activity) {
    const expectedKind: Readonly<Record<CollaborationActivitySemantic, CollaborationEventKind>> = {
      started: 'wake_created',
      updated: 'mailbox_enqueued',
      waiting_approval: 'approval_projected',
      completed: 'wake_updated',
      failed: 'wake_updated',
      interrupted: 'wake_updated'
    }
    const hasValidSubject =
      parsed.activity.semantic === 'updated'
        ? parsed.activity.agentId !== parsed.agentId
        : parsed.activity.agentId === parsed.agentId
    if (parsed.kind !== expectedKind[parsed.activity.semantic] || !hasValidSubject) {
      throw new Error('Invalid CollaborationEventEnvelope activity')
    }
  }
  return parsed
}

export function parseAgentObserverEventEnvelope(value: unknown): AgentObserverEventEnvelope {
  const item = record(value, 'AgentObserverEventEnvelope')
  exact(
    item,
    [
      'schemaVersion',
      'rootAgentId',
      'rootConversationId',
      'agentId',
      'conversationId',
      'runId',
      'assistantMessageId',
      'event'
    ],
    'AgentObserverEventEnvelope'
  )
  const parsed: AgentObserverEventEnvelope = {
    schemaVersion: schema(item.schemaVersion, 'AgentObserverEventEnvelope'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    runId: text(item.runId, 'runId'),
    assistantMessageId: text(item.assistantMessageId, 'assistantMessageId'),
    event: parseObserverAgentEvent(item.event)
  }
  if (parsed.event.runId !== parsed.runId) {
    throw new Error('Invalid AgentObserverEventEnvelope run identity')
  }
  if (
    (parsed.event.type === 'command_started' ||
      parsed.event.type === 'command_output' ||
      parsed.event.type === 'command_exited' ||
      parsed.event.type === 'command_interrupted') &&
    (parsed.event.conversationId !== parsed.conversationId ||
      parsed.event.assistantMessageId !== parsed.assistantMessageId)
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope Command identity')
  }
  if (
    parsed.event.type === 'file_draft_updated' &&
    parsed.event.draft.conversationId !== parsed.conversationId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope file draft identity')
  }
  return parsed
}

const OBSERVER_EVENT_MAX_TEXT_BYTES = 1024 * 1024
const OBSERVER_EVENT_MAX_JSON_BYTES = 4 * 1024 * 1024

function boundedObserverText(value: unknown, context: string, maximumBytes: number): string {
  if (typeof value !== 'string' || new TextEncoder().encode(value).byteLength > maximumBytes) {
    throw new Error(`Invalid ${context}`)
  }
  return value
}

function observerRunId(value: unknown, context: string): string {
  const runId = boundedObserverText(value, context, 2048)
  if (runId.length === 0 || runId.trim() !== runId || /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/u.test(runId)) {
    throw new Error(`Invalid ${context}`)
  }
  return runId
}

function boundedObserverJson(value: unknown, context: string): unknown {
  let encoded: string
  try {
    encoded = JSON.stringify(value)
  } catch {
    throw new Error(`Invalid ${context}`)
  }
  if (new TextEncoder().encode(encoded).byteLength > OBSERVER_EVENT_MAX_JSON_BYTES) {
    throw new Error(`Invalid ${context}`)
  }
  return value
}

function exactObserverEvent(
  item: Record<string, unknown>,
  keys: readonly string[],
  context: string
): void {
  const allowed = new Set(keys)
  const unexpected = Object.keys(item).find((key) => !allowed.has(key))
  if (unexpected) throw new Error(`Invalid ${context}.${unexpected}`)
  for (const required of ['type', 'runId']) {
    if (!Object.hasOwn(item, required)) throw new Error(`Missing ${context}.${required}`)
  }
}

function observerOnlyKeys(
  item: Record<string, unknown>,
  keys: readonly string[],
  context: string
): void {
  const allowed = new Set(keys)
  const unexpected = Object.keys(item).find((key) => !allowed.has(key))
  if (unexpected) throw new Error(`Invalid ${context}.${unexpected}`)
}

/** Strict parser for the process-local observer overlay. */
function parseObserverAgentEvent(value: unknown): AgentEvent {
  const item = record(value, 'AgentObserverEventEnvelope.event')
  const type = text(item.type, 'event.type')
  if (isAgentCommandSessionEventType(type)) return parseAgentCommandSessionEvent(item)
  const runId = observerRunId(item.runId, `${type}.runId`)
  if (!runId) throw new Error(`Invalid ${type}.runId`)

  switch (type) {
    case 'message_delta':
      exactObserverEvent(item, ['type', 'runId', 'streamId', 'delta'], type)
      if (!Object.hasOwn(item, 'delta')) throw new Error(`Missing ${type}.delta`)
      return {
        type,
        runId,
        ...(item.streamId === undefined
          ? {}
          : { streamId: boundedObserverText(item.streamId, `${type}.streamId`, 2048) }),
        delta: boundedObserverText(item.delta, `${type}.delta`, OBSERVER_EVENT_MAX_TEXT_BYTES)
      }
    case 'message':
      exactObserverEvent(item, ['type', 'runId', 'content'], type)
      if (!Object.hasOwn(item, 'content')) throw new Error(`Missing ${type}.content`)
      return {
        type,
        runId,
        content: boundedObserverText(item.content, `${type}.content`, OBSERVER_EVENT_MAX_TEXT_BYTES)
      }
    case 'tool_call':
      exactObserverEvent(item, ['type', 'runId', 'traceSequence', 'call', 'identity'], type)
      if (!Object.hasOwn(item, 'call')) throw new Error(`Missing ${type}.call`)
      {
        const call = parseObserverToolCall(item.call)
        const identity = parseAgentToolIdentityForHost(item.identity)
        const identityToolName =
          identity.type === 'mcp'
            ? identity.provenance.modelToolName
            : identity.type === 'builtin_capability'
              ? identity.modelName
              : identity.toolName
        if (identityToolName !== call.tool) {
          throw new Error(`Invalid ${type}.identity`)
        }
        return {
          type,
          runId,
          traceSequence: integer(item.traceSequence, `${type}.traceSequence`, 0),
          call,
          identity
        }
      }
    case 'tool_result':
      exactObserverEvent(item, ['type', 'runId', 'result'], type)
      if (!Object.hasOwn(item, 'result')) throw new Error(`Missing ${type}.result`)
      return { type, runId, result: parseObserverToolResult(item.result) }
    case 'mcp_tool_invocation_state_changed':
      exactObserverEvent(item, ['type', 'runId', 'invocation'], type)
      if (!Object.hasOwn(item, 'invocation')) throw new Error(`Missing ${type}.invocation`)
      return { type, runId, invocation: parseAgentMcpToolInvocationEvent(item.invocation) }
    case 'skill_activated':
      exactObserverEvent(
        item,
        ['type', 'runId', 'activationRevision', 'activatedBy', 'skill'],
        type
      )
      if (item.activatedBy !== 'user' && item.activatedBy !== 'model') {
        throw new Error(`Invalid ${type}.activatedBy`)
      }
      return {
        type,
        runId,
        activationRevision: boundedObserverText(
          item.activationRevision,
          `${type}.activationRevision`,
          2048
        ),
        activatedBy: item.activatedBy,
        skill: parseObserverSkill(item.skill)
      }
    case 'file_draft_updated':
      exactObserverEvent(item, ['type', 'runId', 'draft'], type)
      if (!Object.hasOwn(item, 'draft')) throw new Error(`Missing ${type}.draft`)
      return { type, runId, draft: parseObserverFileDraft(item.draft) }
    case 'error':
      exactObserverEvent(
        item,
        ['type', 'runId', 'traceSequence', 'message', 'recoverable', 'code', 'details'],
        type
      )
      if (typeof item.recoverable !== 'boolean') throw new Error(`Invalid ${type}.recoverable`)
      return {
        type,
        runId,
        traceSequence:
          item.traceSequence === null
            ? null
            : integer(item.traceSequence, `${type}.traceSequence`, 0),
        message: boundedObserverText(
          item.message,
          `${type}.message`,
          OBSERVER_EVENT_MAX_TEXT_BYTES
        ),
        recoverable: item.recoverable,
        ...(item.code === undefined
          ? {}
          : { code: boundedObserverText(item.code, `${type}.code`, 128) }),
        ...(item.details === undefined
          ? {}
          : { details: boundedObserverJson(item.details, `${type}.details`) })
      }
    case 'done':
      exactObserverEvent(
        item,
        [
          'type',
          'runId',
          'success',
          'status',
          'content',
          'usage',
          'finishReason',
          'proposedActions'
        ],
        type
      )
      if (
        typeof item.success !== 'boolean' ||
        (item.proposedActions !== undefined &&
          (!Array.isArray(item.proposedActions) || item.proposedActions.length !== 0))
      ) {
        throw new Error(`Invalid ${type}`)
      }
      return {
        type,
        runId,
        success: item.success,
        ...(item.status === undefined
          ? {}
          : { status: observerRunStatus(item.status, `${type}.status`) }),
        ...(item.content === undefined
          ? {}
          : {
              content: boundedObserverText(
                item.content,
                `${type}.content`,
                OBSERVER_EVENT_MAX_TEXT_BYTES
              )
            }),
        ...(item.usage === undefined ? {} : { usage: parseObserverUsage(item.usage) }),
        ...(item.finishReason === undefined
          ? {}
          : { finishReason: boundedObserverText(item.finishReason, `${type}.finishReason`, 2048) }),
        ...(item.proposedActions === undefined ? {} : { proposedActions: [] })
      }
    default:
      return parseObserverStructuralEvent(type, runId, item)
  }
}

function parseObserverStructuralEvent(
  type: string,
  runId: string,
  item: Record<string, unknown>
): AgentEvent {
  if (
    type === 'message_stream_reset' ||
    type === 'llm_retry' ||
    type === 'context_compaction_started' ||
    type === 'context_compaction_finished'
  ) {
    const parsed = parseAgentEventForHost(item)
    if (parsed.type !== type) throw new Error(`Invalid Agent observer event type ${type}`)
    return parsed
  }
  if (type === 'state') {
    exactObserverEvent(item, ['type', 'runId', 'state'], type)
    return { type, runId, state: parseObserverState(item.state) }
  }
  if (type === 'message_stream_started') {
    exactObserverEvent(item, ['type', 'runId', 'streamId', 'attempt'], type)
    return {
      type,
      runId,
      streamId: text(item.streamId, `${type}.streamId`, 2048),
      attempt: integer(item.attempt, `${type}.attempt`)
    }
  }
  if (type === 'message_stream_committed') {
    exactObserverEvent(item, ['type', 'runId', 'streamId', 'traceSequence'], type)
    return {
      type,
      runId,
      streamId: text(item.streamId, `${type}.streamId`, 2048),
      traceSequence:
        item.traceSequence === null ? null : integer(item.traceSequence, `${type}.traceSequence`, 0)
    }
  }
  throw new Error(`Invalid Agent observer event type ${type}`)
}

function observerRunStatus(value: unknown, context: string) {
  return oneOf(
    value,
    [
      'idle',
      'queued',
      'running',
      'waiting_for_approval',
      'completed',
      'failed',
      'cancelled'
    ] as const,
    context
  )
}

function parseObserverToolCall(value: unknown): AgentToolCall {
  const item = record(value, 'observer Tool call')
  exact(item, ['id', 'tool', 'args', 'approvalStatus', 'reason'], 'observer Tool call')
  return {
    id: text(item.id, 'observer Tool call.id', 2048),
    tool: text(item.tool, 'observer Tool call.tool', 1024),
    args: boundedObserverJson(item.args, 'observer Tool call.args'),
    approvalStatus: oneOf(
      item.approvalStatus,
      ['not_required', 'required', 'approved', 'rejected'] as const,
      'observer Tool call.approvalStatus'
    ),
    reason:
      item.reason === null
        ? null
        : boundedObserverText(item.reason, 'observer Tool call.reason', 16 * 1024)
  }
}

function parseObserverToolResult(value: unknown): AgentToolResult {
  const item = record(value, 'observer Tool result')
  const allowed = ['callId', 'tool', 'ok', 'result', 'error'] as const
  observerOnlyKeys(item, allowed, 'observer Tool result')
  if (
    !Object.hasOwn(item, 'callId') ||
    !Object.hasOwn(item, 'tool') ||
    !Object.hasOwn(item, 'ok')
  ) {
    throw new Error('Invalid observer Tool result')
  }
  if (typeof item.ok !== 'boolean') throw new Error('Invalid observer Tool result.ok')
  return {
    callId: text(item.callId, 'observer Tool result.callId', 2048),
    tool: text(item.tool, 'observer Tool result.tool', 1024),
    ok: item.ok,
    ...(item.result === undefined
      ? {}
      : { result: boundedObserverJson(item.result, 'observer Tool result.result') }),
    ...(item.error === undefined
      ? {}
      : {
          error: boundedObserverText(
            item.error,
            'observer Tool result.error',
            OBSERVER_EVENT_MAX_TEXT_BYTES
          )
        })
  }
}

function parseObserverSkill(value: unknown): ActivatedSkillSummary {
  const item = record(value, 'observer Skill')
  exact(item, ['id', 'name', 'revision', 'source'], 'observer Skill')
  const source = record(item.source, 'observer Skill.source')
  exact(source, ['kind', 'id'], 'observer Skill.source')
  return {
    id: text(item.id, 'observer Skill.id'),
    name: text(item.name, 'observer Skill.name', 1024),
    revision: text(item.revision, 'observer Skill.revision', 2048),
    source: {
      kind: oneOf(
        source.kind,
        ['workspace', 'bundled', 'installed'] as const,
        'observer Skill.source.kind'
      ),
      id: text(source.id, 'observer Skill.source.id')
    }
  }
}

function parseObserverFileDraft(value: unknown): AgentFileDraftSnapshot {
  const item = record(value, 'observer file draft')
  const keys = [
    'draftId',
    'conversationId',
    'projectId',
    'filePath',
    'mode',
    'status',
    'baseRevision',
    'additions',
    'deletions',
    'lineCount',
    'byteCount',
    'chunkCount',
    'nextChunkIndex',
    'statsFinal',
    'summary',
    'createdAt',
    'updatedAt'
  ] as const
  observerOnlyKeys(item, keys, 'observer file draft')
  for (const required of [
    'draftId',
    'conversationId',
    'filePath',
    'mode',
    'status',
    'additions',
    'deletions',
    'lineCount',
    'byteCount',
    'chunkCount',
    'nextChunkIndex',
    'statsFinal',
    'createdAt',
    'updatedAt'
  ]) {
    if (!Object.hasOwn(item, required)) throw new Error(`Invalid observer file draft.${required}`)
  }
  if (typeof item.statsFinal !== 'boolean')
    throw new Error('Invalid observer file draft.statsFinal')
  return {
    draftId: text(item.draftId, 'observer file draft.draftId'),
    conversationId: text(item.conversationId, 'observer file draft.conversationId'),
    ...(item.projectId === undefined
      ? {}
      : { projectId: text(item.projectId, 'observer file draft.projectId') }),
    filePath: text(item.filePath, 'observer file draft.filePath', 16 * 1024),
    mode: oneOf(
      item.mode,
      ['create', 'rewrite', 'modify', 'append', 'upsert'] as const,
      'observer file draft.mode'
    ),
    status: oneOf(
      item.status,
      [
        'drafting',
        'ready',
        'waiting_approval',
        'applying',
        'applied',
        'rejected',
        'conflict',
        'failed',
        'aborted',
        'expired'
      ] as const,
      'observer file draft.status'
    ),
    ...(item.baseRevision === undefined
      ? {}
      : { baseRevision: text(item.baseRevision, 'observer file draft.baseRevision', 2048) }),
    additions: integer(item.additions, 'observer file draft.additions'),
    deletions: integer(item.deletions, 'observer file draft.deletions'),
    lineCount: integer(item.lineCount, 'observer file draft.lineCount'),
    byteCount: integer(item.byteCount, 'observer file draft.byteCount'),
    chunkCount: integer(item.chunkCount, 'observer file draft.chunkCount'),
    nextChunkIndex: integer(item.nextChunkIndex, 'observer file draft.nextChunkIndex'),
    statsFinal: item.statsFinal,
    ...(item.summary === undefined
      ? {}
      : { summary: boundedObserverText(item.summary, 'observer file draft.summary', 16 * 1024) }),
    createdAt: integer(item.createdAt, 'observer file draft.createdAt'),
    updatedAt: integer(item.updatedAt, 'observer file draft.updatedAt')
  }
}

function parseObserverState(value: unknown): AgentStateSnapshot {
  const item = record(value, 'observer state')
  exact(item, ['status', 'activeRunId', 'lastError', 'updatedAt'], 'observer state')
  return {
    status: observerRunStatus(item.status, 'observer state.status'),
    activeRunId: item.activeRunId === null ? null : observerRunId(item.activeRunId, 'activeRunId'),
    lastError:
      item.lastError === null
        ? null
        : boundedObserverText(
            item.lastError,
            'observer state.lastError',
            OBSERVER_EVENT_MAX_TEXT_BYTES
          ),
    updatedAt: integer(item.updatedAt, 'observer state.updatedAt')
  }
}

function parseObserverUsage(value: unknown): AgentUsage {
  const item = record(value, 'observer usage')
  const keys = [
    'inputTokens',
    'outputTokens',
    'outputThinkingTokens',
    'totalTokens',
    'cachedInputTokens',
    'cacheCreationInputTokens',
    'billableRequestCount'
  ] as const
  observerOnlyKeys(item, keys, 'observer usage')
  const usage: AgentUsage = {}
  for (const key of keys) {
    if (item[key] !== undefined) usage[key] = integer(item[key], `observer usage.${key}`)
  }
  return usage
}

export function parseCollaborationEventsRequest(value: unknown): CollaborationEventsRequest {
  const item = record(value, 'CollaborationEventsRequest')
  exact(item, ['rootConversationId', 'afterSequence', 'limit'], 'CollaborationEventsRequest')
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    afterSequence: integer(item.afterSequence, 'afterSequence'),
    limit: integer(item.limit, 'limit', 1, MAX_EVENT_PAGE)
  }
}

export function parseCollaborationEventsPage(value: unknown): CollaborationEventsPage {
  const item = record(value, 'CollaborationEventsPage')
  exact(
    item,
    ['schemaVersion', 'rootAgentId', 'rootConversationId', 'events', 'lastSequence', 'hasMore'],
    'CollaborationEventsPage'
  )
  if (!Array.isArray(item.events) || item.events.length > MAX_EVENT_PAGE)
    throw new Error('Invalid events')
  const parsed = {
    schemaVersion: schema(item.schemaVersion, 'CollaborationEventsPage'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    events: item.events.map(parseCollaborationEventEnvelope),
    lastSequence: integer(item.lastSequence, 'lastSequence'),
    hasMore: bool(item.hasMore, 'hasMore')
  }
  if (
    parsed.events.some(
      (event, index) =>
        event.rootAgentId !== parsed.rootAgentId ||
        event.rootConversationId !== parsed.rootConversationId ||
        (index > 0 && event.sequence !== parsed.events[index - 1]!.sequence + 1)
    ) ||
    (parsed.events.length > 0 && parsed.lastSequence !== parsed.events.at(-1)?.sequence)
  ) {
    throw new Error('Invalid CollaborationEventsPage identity')
  }
  return parsed
}

export function parseCollaborationResyncEnvelope(value: unknown): CollaborationResyncEnvelope {
  const item = record(value, 'CollaborationResyncEnvelope')
  exact(item, ['schemaVersion', 'reason'], 'CollaborationResyncEnvelope')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationResyncEnvelope'),
    reason: oneOf(item.reason, ['core_started', 'model_settings_changed'] as const, 'reason')
  }
}

export function parseAgentTemplate(value: unknown): AgentTemplate {
  const item = record(value, 'AgentTemplate')
  exact(
    item,
    [
      'schemaVersion',
      'templateId',
      'projectId',
      'machineKey',
      'name',
      'description',
      'instructions',
      'modelConfigId',
      'modelDisplayName',
      'enabled',
      'revision',
      'createdAt',
      'updatedAt'
    ],
    'AgentTemplate'
  )
  if (typeof item.description !== 'string' || typeof item.instructions !== 'string')
    throw new Error('Invalid template text')
  if (
    new TextEncoder().encode(item.description).byteLength > MAX_TEXT_BYTES ||
    new TextEncoder().encode(item.instructions).byteLength > MAX_TEXT_BYTES
  )
    throw new Error('Invalid template text')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTemplate'),
    templateId: text(item.templateId, 'templateId'),
    projectId: text(item.projectId, 'projectId'),
    machineKey: text(item.machineKey, 'machineKey'),
    name: text(item.name, 'name'),
    description: item.description,
    instructions: item.instructions,
    modelConfigId: text(item.modelConfigId, 'modelConfigId', 1_024),
    modelDisplayName: nullableText(item.modelDisplayName, 'modelDisplayName'),
    enabled: bool(item.enabled, 'enabled'),
    revision: integer(item.revision, 'revision', 1),
    createdAt: integer(item.createdAt, 'createdAt'),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseAgentTemplateList(value: unknown): AgentTemplateList {
  const item = record(value, 'AgentTemplateList')
  exact(item, ['schemaVersion', 'templates'], 'AgentTemplateList')
  if (!Array.isArray(item.templates) || item.templates.length > 256)
    throw new Error('Invalid templates')
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTemplateList'),
    templates: item.templates.map(parseAgentTemplate)
  }
}

export function parseAgentTemplateListRequest(value: unknown): AgentTemplateListRequest {
  const item = record(value, 'AgentTemplateListRequest')
  exact(item, ['projectId', 'includeDisabled'], 'AgentTemplateListRequest')
  return {
    projectId: text(item.projectId, 'projectId'),
    includeDisabled: bool(item.includeDisabled, 'includeDisabled')
  }
}

export function parseAgentTemplateCreateRequest(value: unknown): AgentTemplateCreateRequest {
  const item = record(value, 'AgentTemplateCreateRequest')
  exact(
    item,
    [
      'templateId',
      'projectId',
      'machineKey',
      'name',
      'description',
      'instructions',
      'modelConfigId',
      'enabled'
    ],
    'AgentTemplateCreateRequest'
  )
  const description = typeof item.description === 'string' ? item.description : null
  const instructions = typeof item.instructions === 'string' ? item.instructions : null
  if (
    description === null ||
    instructions === null ||
    new TextEncoder().encode(description).byteLength > MAX_TEXT_BYTES ||
    new TextEncoder().encode(instructions).byteLength > MAX_TEXT_BYTES
  ) {
    throw new Error('Invalid AgentTemplateCreateRequest text')
  }
  return {
    templateId: text(item.templateId, 'templateId'),
    projectId: text(item.projectId, 'projectId'),
    machineKey: text(item.machineKey, 'machineKey'),
    name: text(item.name, 'name'),
    description,
    instructions,
    modelConfigId: text(item.modelConfigId, 'modelConfigId', 1_024),
    enabled: bool(item.enabled, 'enabled')
  }
}

export function parseAgentTemplateUpdateRequest(value: unknown): AgentTemplateUpdateRequest {
  const item = record(value, 'AgentTemplateUpdateRequest')
  exact(
    item,
    [
      'projectId',
      'templateId',
      'expectedRevision',
      'name',
      'description',
      'instructions',
      'modelConfigId'
    ],
    'AgentTemplateUpdateRequest'
  )
  const common = parseAgentTemplateCreateRequest({
    templateId: item.templateId,
    projectId: item.projectId,
    machineKey: 'immutable-machine-key',
    name: item.name,
    description: item.description,
    instructions: item.instructions,
    modelConfigId: item.modelConfigId,
    enabled: true
  })
  return {
    projectId: common.projectId,
    templateId: common.templateId,
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1),
    name: common.name,
    description: common.description,
    instructions: common.instructions,
    modelConfigId: common.modelConfigId
  }
}

export function parseAgentTemplateSetEnabledRequest(
  value: unknown
): AgentTemplateSetEnabledRequest {
  const item = record(value, 'AgentTemplateSetEnabledRequest')
  exact(
    item,
    ['projectId', 'templateId', 'expectedRevision', 'enabled'],
    'AgentTemplateSetEnabledRequest'
  )
  return {
    projectId: text(item.projectId, 'projectId'),
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1),
    enabled: bool(item.enabled, 'enabled')
  }
}

export function parseAgentTemplateDeleteRequest(value: unknown): AgentTemplateDeleteRequest {
  const item = record(value, 'AgentTemplateDeleteRequest')
  exact(item, ['projectId', 'templateId', 'expectedRevision'], 'AgentTemplateDeleteRequest')
  return {
    projectId: text(item.projectId, 'projectId'),
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1)
  }
}

export function parseCollaborationApprovalListRequest(
  value: unknown
): CollaborationApprovalListRequest {
  return parseAgentTreeRequest(value)
}

export function parseCollaborationApprovalProjection(
  value: unknown
): CollaborationApprovalProjection {
  const item = record(value, 'CollaborationApprovalProjection')
  exact(
    item,
    [
      'schemaVersion',
      'approvalId',
      'rootAgentId',
      'rootConversationId',
      'sourceAgentId',
      'sourceTaskPath',
      'sourceConversationId',
      'runId',
      'actionId',
      'actionType',
      'toolName',
      'action',
      'status',
      'createdAt',
      'updatedAt'
    ],
    'CollaborationApprovalProjection'
  )
  const actionId = text(item.actionId, 'actionId')
  const actionType = text(item.actionType, 'actionType')
  const toolName = text(item.toolName, 'toolName')
  const runId = text(item.runId, 'runId', 2_048)
  const sourceConversationId = text(item.sourceConversationId, 'sourceConversationId')
  const createdAt = integer(item.createdAt, 'createdAt')
  const actionRecord = record(item.action, 'CollaborationApprovalProjection.action')
  const protectedToolCallId =
    actionRecord.type === 'builtin_capability_activation'
      ? parseAgentBuiltinCapabilityActivationProposedAction(actionRecord).approval.callId
      : actionRecord.type === 'browser_risk_approval'
        ? parseAgentBrowserRiskProposedAction(actionRecord).approval.callId
        : null
  const status = oneOf(
    item.status,
    [
      'pending',
      'approved',
      'executing',
      'rejected',
      'cancelled',
      'completed',
      'failed',
      'expired',
      'interrupted'
    ] as const,
    'status'
  )
  const parsedAction = parsePendingAgentActionSnapshotsForHost([
    {
      actionId,
      actionType,
      toolName,
      // Host-owned capability approvals are bound to the exact model Tool call. Passing null here
      // would make a collaboration projection weaker than the authoritative pending snapshot and
      // would also prevent the strict pending-action parser from hydrating it.
      toolCallId: protectedToolCallId,
      runId,
      conversationId: sourceConversationId,
      assistantMessageId: null,
      action: item.action,
      createdAt,
      status: status === 'expired' || status === 'interrupted' ? 'failed' : status
    }
  ])[0]?.action
  if (!parsedAction) throw new Error('Invalid approval action')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalProjection'),
    approvalId: text(item.approvalId, 'approvalId'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    sourceAgentId: text(item.sourceAgentId, 'sourceAgentId'),
    sourceTaskPath: text(item.sourceTaskPath, 'sourceTaskPath', 4_096),
    sourceConversationId,
    runId,
    actionId,
    actionType,
    toolName,
    action: parsedAction,
    status,
    createdAt,
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}

export function parseCollaborationApprovalList(value: unknown): CollaborationApprovalList {
  const item = record(value, 'CollaborationApprovalList')
  exact(item, ['schemaVersion', 'approvals'], 'CollaborationApprovalList')
  if (!Array.isArray(item.approvals) || item.approvals.length > 1_024)
    throw new Error('Invalid approvals')
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalList'),
    approvals: item.approvals.map(parseCollaborationApprovalProjection)
  }
}

export function parseCollaborationApprovalDecisionRequest(
  value: unknown
): CollaborationApprovalDecisionRequest {
  const item = record(value, 'CollaborationApprovalDecisionRequest')
  exact(
    item,
    ['rootConversationId', 'approvalId', 'decision', 'message'],
    'CollaborationApprovalDecisionRequest'
  )
  return {
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    approvalId: text(item.approvalId, 'approvalId'),
    decision: oneOf(item.decision, ['approve', 'reject', 'cancel'] as const, 'decision'),
    message: item.message === null ? null : text(item.message, 'message', 4_096)
  }
}

export function parseCollaborationApprovalDecisionResult(
  value: unknown
): CollaborationApprovalDecisionResult {
  const item = record(value, 'CollaborationApprovalDecisionResult')
  exact(
    item,
    ['schemaVersion', 'approvalId', 'accepted', 'alreadySettled', 'status'],
    'CollaborationApprovalDecisionResult'
  )
  return {
    schemaVersion: schema(item.schemaVersion, 'CollaborationApprovalDecisionResult'),
    approvalId: text(item.approvalId, 'approvalId'),
    accepted: bool(item.accepted, 'accepted'),
    alreadySettled: bool(item.alreadySettled, 'alreadySettled'),
    status: oneOf(
      item.status,
      [
        'pending',
        'approved',
        'executing',
        'rejected',
        'cancelled',
        'completed',
        'failed',
        'expired',
        'interrupted'
      ] as const,
      'status'
    )
  }
}
