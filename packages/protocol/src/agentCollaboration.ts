import type { AgentEvent, AgentProposedAction } from './agent'
import type { HumanInteractionResponseDisplay } from './humanInteraction'
import { parseStorageHumanInteractionResponse } from './storageHumanInteraction'
import {
  parseAgentBrowserRiskProposedAction,
  parseAgentBuiltinCapabilityActivationProposedAction
} from './agentParsers/builtinApprovals'
import { parseAgentEventForHost } from './agentParsers/events'
import { parsePendingAgentActionSnapshotsForHost } from './agentParsers/pendingActions'

export const AGENT_COLLABORATION_SCHEMA_VERSION = 1 as const
export const AGENT_COLLABORATION_EVENT_SCHEMA_VERSION = 2 as const
export const AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION = 3 as const

export const AGENT_COLLABORATION_GET_SETTINGS_METHOD = 'agent.collaboration.settings.get'
export const AGENT_COLLABORATION_UPDATE_SETTINGS_METHOD = 'agent.collaboration.settings.update'
export const AGENT_COLLABORATION_SETTINGS_CHANGED_METHOD = 'agent.collaboration.settingsChanged'

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

export function parseAgentCollaborationSettingsGetInput(
  value: unknown
): AgentCollaborationSettingsGetInput {
  const item = record(value, 'AgentCollaborationSettingsGetInput')
  exact(item, [], 'AgentCollaborationSettingsGetInput')
  return {}
}
export function parseAgentCollaborationSettings(value: unknown): AgentCollaborationSettings {
  const item = record(value, 'AgentCollaborationSettings')
  exact(item, ['enabled', 'revision', 'updatedAt'], 'AgentCollaborationSettings')
  return {
    enabled: bool(item.enabled, 'enabled'),
    revision: integer(item.revision, 'revision', 1),
    updatedAt: integer(item.updatedAt, 'updatedAt')
  }
}
export function parseAgentCollaborationSettingsUpdate(
  value: unknown
): AgentCollaborationSettingsUpdate {
  const item = record(value, 'AgentCollaborationSettingsUpdate')
  exact(item, ['enabled', 'expectedRevision'], 'AgentCollaborationSettingsUpdate')
  return {
    enabled: bool(item.enabled, 'enabled'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1)
  }
}

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
export const AGENT_COLLABORATION_TEMPLATES_SET_PROJECT_ASSIGNMENT_METHOD =
  'agent.collaboration.templates.setProjectAssignment'
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
  semantic: CollaborationActivitySemantic
  /** Presentation subject; the outer event agent remains the invalidation subject. */
  agentId: string
  taskNameSnapshot: string
  /** Direct parent and the only Conversation that presents this activity. */
  parentAgentId: string
  parentConversationId: string
  /** Parent message containing or preceding the activity; null means before its first message. */
  anchorMessageId: string | null
  /** Insert inside the active parent trace at this boundary; null means after the anchor message. */
  traceBoundarySequence: number | null
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

function utf8LessThan(left: string, right: string): boolean {
  const leftBytes = new TextEncoder().encode(left)
  const rightBytes = new TextEncoder().encode(right)
  const sharedLength = Math.min(leftBytes.length, rightBytes.length)
  for (let index = 0; index < sharedLength; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) return leftBytes[index]! < rightBytes[index]!
  }
  return leftBytes.length < rightBytes.length
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

function parseObserverStreamCursor(value: unknown): AgentObserverStreamCursor {
  const item = record(value, 'AgentObserverStreamCursor')
  exact(item, ['generation', 'sequence'], 'AgentObserverStreamCursor')
  return {
    generation: text(item.generation, 'AgentObserverStreamCursor.generation'),
    sequence: integer(item.sequence, 'AgentObserverStreamCursor.sequence', 1)
  }
}

function parseObserverLiveStream(value: unknown): AgentObserverLiveStreamSnapshot {
  const item = record(value, 'AgentObserverLiveStreamSnapshot')
  exact(
    item,
    ['runId', 'assistantMessageId', 'cursor', 'stream'],
    'AgentObserverLiveStreamSnapshot'
  )
  let stream: AgentObserverLiveStreamSnapshot['stream'] = null
  if (item.stream !== null) {
    const value = record(item.stream, 'AgentObserverLiveStream')
    exact(
      value,
      ['streamId', 'attempt', 'content', 'traceBoundarySequence', 'committed'],
      'AgentObserverLiveStream'
    )
    if (typeof value.content !== 'string')
      throw new Error('Invalid AgentObserverLiveStream.content')
    stream = {
      streamId: text(value.streamId, 'AgentObserverLiveStream.streamId', 1_024),
      attempt: integer(value.attempt, 'AgentObserverLiveStream.attempt', 1),
      content: value.content,
      traceBoundarySequence: integer(
        value.traceBoundarySequence,
        'AgentObserverLiveStream.traceBoundarySequence'
      ),
      committed: bool(value.committed, 'AgentObserverLiveStream.committed')
    }
  }
  return {
    runId: text(item.runId, 'AgentObserverLiveStreamSnapshot.runId'),
    assistantMessageId: text(
      item.assistantMessageId,
      'AgentObserverLiveStreamSnapshot.assistantMessageId'
    ),
    cursor: parseObserverStreamCursor(item.cursor),
    stream
  }
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
      'messages',
      ...('liveStream' in item ? ['liveStream'] : [])
    ],
    'AgentObserverConversation'
  )
  if (!Array.isArray(item.messages) || item.messages.length > 100_000) {
    throw new Error('Invalid AgentObserverConversation.messages')
  }
  const parsed: AgentObserverConversation = {
    ...(item.liveStream === undefined
      ? {}
      : { liveStream: parseObserverLiveStream(item.liveStream) }),
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
          'uiStateJson',
          ...('humanInteractionResponse' in message ? ['humanInteractionResponse'] : [])
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
        ...(message.humanInteractionResponse == null
          ? {}
          : {
              humanInteractionResponse: parseStorageHumanInteractionResponse({
                role,
                content: message.content,
                humanInteractionResponse: message.humanInteractionResponse
              })
            }),
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
  if (
    parsed.liveStream &&
    !parsed.messages.some(
      (message) =>
        message.messageId === parsed.liveStream!.assistantMessageId &&
        message.role === 'assistant' &&
        message.agentRunJson !== null &&
        (JSON.parse(message.agentRunJson) as { runId?: unknown })?.runId ===
          parsed.liveStream!.runId
    )
  ) {
    throw new Error('Invalid AgentObserverLiveStreamSnapshot message identity')
  }
  return parsed
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
              'parentAgentId',
              'parentConversationId',
              'anchorMessageId',
              'traceBoundarySequence'
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
            parentAgentId: text(
              snapshot.parentAgentId,
              'CollaborationActivitySnapshot.parentAgentId'
            ),
            parentConversationId: text(
              snapshot.parentConversationId,
              'CollaborationActivitySnapshot.parentConversationId'
            ),
            anchorMessageId: nullableText(
              snapshot.anchorMessageId,
              'CollaborationActivitySnapshot.anchorMessageId'
            ),
            traceBoundarySequence:
              snapshot.traceBoundarySequence === null
                ? null
                : integer(
                    snapshot.traceBoundarySequence,
                    'CollaborationActivitySnapshot.traceBoundarySequence',
                    0
                  )
          } satisfies CollaborationActivitySnapshot
        })()
  if (
    activity !== null &&
    activity.anchorMessageId === null &&
    activity.traceBoundarySequence !== null
  ) {
    throw new Error(
      'Invalid CollaborationActivitySnapshot: trace placement requires an anchor message'
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
    const activity = parsed.activity
    const parentIsRoot = activity.parentAgentId === parsed.rootAgentId
    const parentConversationIsRoot = activity.parentConversationId === parsed.rootConversationId
    const hasValidParent =
      activity.parentAgentId !== activity.agentId &&
      parentIsRoot === parentConversationIsRoot &&
      (activity.semantic === 'updated'
        ? activity.parentAgentId === parsed.agentId &&
          activity.parentConversationId === parsed.conversationId
        : activity.parentConversationId !== parsed.conversationId)
    if (!hasValidParent) {
      throw new Error('Invalid CollaborationEventEnvelope activity parent identity')
    }
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
      'event',
      ...('streamCursor' in item ? ['streamCursor'] : [])
    ],
    'AgentObserverEventEnvelope'
  )
  const runId = text(item.runId, 'runId')
  const event = parseAgentEventForHost(item.event)
  const boundEvent: AgentEvent =
    event.type === 'error' && event.runId === undefined ? { ...event, runId } : event
  const parsed: AgentObserverEventEnvelope = {
    ...(item.streamCursor === undefined
      ? {}
      : { streamCursor: parseObserverStreamCursor(item.streamCursor) }),
    schemaVersion: schema(item.schemaVersion, 'AgentObserverEventEnvelope'),
    rootAgentId: text(item.rootAgentId, 'rootAgentId'),
    rootConversationId: text(item.rootConversationId, 'rootConversationId'),
    agentId: text(item.agentId, 'agentId'),
    conversationId: text(item.conversationId, 'conversationId'),
    runId,
    assistantMessageId: text(item.assistantMessageId, 'assistantMessageId'),
    event: boundEvent
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
    parsed.event.type === 'file_change_updated' &&
    parsed.event.fileChange.conversationId !== parsed.conversationId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope FileChange identity')
  }
  if (
    parsed.event.type === 'context_window_updated' &&
    parsed.event.conversationId !== undefined &&
    parsed.event.conversationId !== parsed.conversationId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope context window identity')
  }
  if (
    parsed.event.type === 'state' &&
    parsed.event.state.activeRunId !== null &&
    parsed.event.state.activeRunId !== parsed.runId
  ) {
    throw new Error('Invalid AgentObserverEventEnvelope state identity')
  }
  return parsed
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
      'projectIds',
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
  if (!Array.isArray(item.projectIds) || item.projectIds.length > 256) {
    throw new Error('Invalid AgentTemplate.projectIds')
  }
  const projectIds = item.projectIds.map((projectId, index) =>
    text(projectId, `AgentTemplate.projectIds[${index}]`)
  )
  if (
    projectIds.some(
      (projectId, index) => index > 0 && !utf8LessThan(projectIds[index - 1]!, projectId)
    )
  ) {
    throw new Error('Invalid AgentTemplate.projectIds')
  }
  return {
    schemaVersion: schema(item.schemaVersion, 'AgentTemplate'),
    templateId: text(item.templateId, 'templateId'),
    projectIds,
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
  exact(item, ['includeDisabled'], 'AgentTemplateListRequest')
  return {
    includeDisabled: bool(item.includeDisabled, 'includeDisabled')
  }
}

export function parseAgentTemplateCreateRequest(value: unknown): AgentTemplateCreateRequest {
  const item = record(value, 'AgentTemplateCreateRequest')
  exact(
    item,
    ['templateId', 'machineKey', 'name', 'description', 'instructions', 'modelConfigId', 'enabled'],
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
    ['templateId', 'expectedRevision', 'name', 'description', 'instructions', 'modelConfigId'],
    'AgentTemplateUpdateRequest'
  )
  const common = parseAgentTemplateCreateRequest({
    templateId: item.templateId,
    machineKey: 'immutable-machine-key',
    name: item.name,
    description: item.description,
    instructions: item.instructions,
    modelConfigId: item.modelConfigId,
    enabled: true
  })
  return {
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
  exact(item, ['templateId', 'expectedRevision', 'enabled'], 'AgentTemplateSetEnabledRequest')
  return {
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1),
    enabled: bool(item.enabled, 'enabled')
  }
}

export function parseAgentTemplateDeleteRequest(value: unknown): AgentTemplateDeleteRequest {
  const item = record(value, 'AgentTemplateDeleteRequest')
  exact(item, ['templateId', 'expectedRevision'], 'AgentTemplateDeleteRequest')
  return {
    templateId: text(item.templateId, 'templateId'),
    expectedRevision: integer(item.expectedRevision, 'expectedRevision', 1)
  }
}

export function parseAgentTemplateProjectAssignmentRequest(
  value: unknown
): AgentTemplateProjectAssignmentRequest {
  const item = record(value, 'AgentTemplateProjectAssignmentRequest')
  exact(item, ['projectId', 'templateId', 'assigned'], 'AgentTemplateProjectAssignmentRequest')
  return {
    projectId: text(item.projectId, 'projectId'),
    templateId: text(item.templateId, 'templateId'),
    assigned: bool(item.assigned, 'assigned')
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
