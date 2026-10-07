/** Host-owned organization mail delivery facts; notification cursors never grant execution authority. */
export const WORKFLOW_RUNTIME_CHANGED_METHOD = 'agent.workflows.runtime.changed'

export interface WorkflowSourceMessage {
  id: string
  instanceId: string
  workflowName: string
  sourceNodeId: string
  sourceNodeName: string
  sourceConversationId: string
  sourceConversationTitle: string
  targetNodeId: string
  targetNodeName: string
  targetConversationId: string | null
  targetConversationTitle: string | null
  replyToMessageId: string | null
  content: string
  createdAt: number
}
export type WorkflowInputStatus =
  | 'pending'
  | 'claimed'
  | 'applied'
  | 'completed'
  | 'paused'
  | 'failed'
  | 'invalidated'
  | 'stopped'
  | 'recalled'
export type WorkflowMailStatus =
  'pending' | 'processing' | 'processed' | 'stopped' | 'failed' | 'recalled'
export interface WorkflowRuntimeInput {
  id: string
  instanceId: string
  nodeId: string
  conversationId: string | null
  executionVersion: string
  content: string
  messages: WorkflowSourceMessage[]
  mailStatus: WorkflowMailStatus
  status: WorkflowInputStatus
  runId: string | null
  deliveryId: string | null
  createdAt: number
  error: string | null
}
export interface WorkflowRuntimeEvent {
  sequence: number
  instanceId: string
  inputId: string | null
  messageId: string | null
  sourceNodeId: string | null
  targetNodeId: string | null
  kind: string
  createdAt: number
}
export interface WorkflowRuntimeSnapshot {
  instanceId: string
  sequence: number
  inputs: WorkflowRuntimeInput[]
  events: WorkflowRuntimeEvent[]
  pausedConversationIds?: string[]
  inputRuns?: { inputId: string; status: string }[]
  /** Present only on a fresh edit notification, never on historical/replayed snapshots. */
  preferenceUpdates?: WorkflowPreferenceUpdate[]
  /** Metadata projection; historical bodies/envelopes remain available through explicit reads. */
  summary?: WorkflowRuntimeSummary
}
export interface WorkflowRuntimeSummary {
  pendingByNode: { nodeId: string; count: number }[]
  conversationChanges: { conversationId: string; sequence: number }[]
  /** Durable management event sequence, independent of the recent event window. */
  structureRevision: number
}
export interface WorkflowPreferenceUpdate {
  nodeId: string
  conversationId: string
  organizationRevision: number
  modelId?: string
  permissionMode?: 'default' | 'custom' | 'full'
}
export interface WorkflowMessageSource {
  inputId: string
  instanceId: string
  workflowName: string
  sources: {
    nodeId: string
    nodeName: string
    conversationId: string
    conversationTitle: string
    /** Original collaborator body from the Host receipt, separate from the assembled context. */
    content?: string
  }[]
}

function record(
  value: unknown,
  keys: readonly string[],
  optionalKeys: readonly string[] = []
): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid organization runtime object')
  const item = value as Record<string, unknown>
  if (
    Object.keys(item).some((key) => !keys.includes(key) && !optionalKeys.includes(key)) ||
    keys.some((key) => !(key in item))
  )
    throw new Error('Invalid organization runtime fields')
  return item
}
function text(value: unknown, max = 2_000_000): string {
  if (typeof value !== 'string' || value.length > max)
    throw new Error('Invalid organization runtime text')
  return value
}
function id(value: unknown): string {
  const result = text(value, 512)
  if (
    !result.trim() ||
    [...result].some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127)
  )
    throw new Error('Invalid organization runtime ID')
  return result
}
function integer(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('Invalid organization runtime sequence or time')
  return value
}
function list<T>(value: unknown, parse: (item: unknown) => T, max = 1024): T[] {
  if (!Array.isArray(value) || value.length > max)
    throw new Error('Invalid organization runtime collection')
  return value.map(parse)
}
function nullableId(value: unknown): string | null {
  return value === null ? null : id(value)
}

export function parseWorkflowSourceMessage(value: unknown): WorkflowSourceMessage {
  const item = record(value, [
    'id',
    'instanceId',
    'workflowName',
    'sourceNodeId',
    'sourceNodeName',
    'sourceConversationId',
    'sourceConversationTitle',
    'targetNodeId',
    'targetNodeName',
    'targetConversationId',
    'targetConversationTitle',
    'replyToMessageId',
    'content',
    'createdAt'
  ])
  return {
    id: id(item.id),
    instanceId: id(item.instanceId),
    workflowName: text(item.workflowName, 512),
    sourceNodeId: id(item.sourceNodeId),
    sourceNodeName: text(item.sourceNodeName, 512),
    sourceConversationId: id(item.sourceConversationId),
    sourceConversationTitle: text(item.sourceConversationTitle, 4096),
    targetNodeId: id(item.targetNodeId),
    targetNodeName: text(item.targetNodeName, 512),
    targetConversationId: nullableId(item.targetConversationId),
    targetConversationTitle:
      item.targetConversationTitle === null ? null : text(item.targetConversationTitle, 4096),
    replyToMessageId: nullableId(item.replyToMessageId),
    content: text(item.content),
    createdAt: integer(item.createdAt)
  }
}
export function parseWorkflowRuntimeInput(value: unknown): WorkflowRuntimeInput {
  const item = record(value, [
    'id',
    'instanceId',
    'nodeId',
    'conversationId',
    'executionVersion',
    'content',
    'messages',
    'mailStatus',
    'status',
    'runId',
    'deliveryId',
    'createdAt',
    'error'
  ])
  if (
    !['pending', 'processing', 'processed', 'stopped', 'failed', 'recalled'].includes(
      String(item.mailStatus)
    )
  )
    throw new Error('Invalid organization mail status')
  const statuses: string[] = [
    'pending',
    'claimed',
    'applied',
    'completed',
    'paused',
    'failed',
    'invalidated',
    'stopped',
    'recalled'
  ]
  if (typeof item.status !== 'string' || !statuses.includes(item.status))
    throw new Error('Invalid organization input status')
  const input: WorkflowRuntimeInput = {
    id: id(item.id),
    instanceId: id(item.instanceId),
    nodeId: id(item.nodeId),
    conversationId: nullableId(item.conversationId),
    executionVersion: id(item.executionVersion),
    content: text(item.content),
    messages: list(item.messages, parseWorkflowSourceMessage, 512),
    mailStatus: item.mailStatus as WorkflowMailStatus,
    status: item.status as WorkflowInputStatus,
    runId: nullableId(item.runId),
    deliveryId: nullableId(item.deliveryId),
    createdAt: integer(item.createdAt),
    error: item.error === null ? null : text(item.error)
  }
  if (
    input.messages.length !== 1 ||
    input.messages.some(
      (message) => message.instanceId !== input.instanceId || message.targetNodeId !== input.nodeId
    )
  )
    throw new Error('Organization input source identity mismatch')
  return input
}
export function parseWorkflowRuntimeEvent(value: unknown): WorkflowRuntimeEvent {
  const item = record(value, [
    'sequence',
    'instanceId',
    'inputId',
    'messageId',
    'sourceNodeId',
    'targetNodeId',
    'kind',
    'createdAt'
  ])
  return {
    sequence: integer(item.sequence),
    instanceId: id(item.instanceId),
    inputId: nullableId(item.inputId),
    messageId: nullableId(item.messageId),
    sourceNodeId: nullableId(item.sourceNodeId),
    targetNodeId: nullableId(item.targetNodeId),
    kind: id(item.kind),
    createdAt: integer(item.createdAt)
  }
}
export function parseWorkflowRuntimeSnapshot(value: unknown): WorkflowRuntimeSnapshot {
  const item = record(
    value,
    ['instanceId', 'sequence', 'inputs', 'events'],
    ['pausedConversationIds', 'inputRuns', 'preferenceUpdates', 'summary']
  )
  const result = {
    instanceId: id(item.instanceId),
    sequence: integer(item.sequence),
    inputs: list(item.inputs, parseWorkflowRuntimeInput, 4096),
    events: list(item.events, parseWorkflowRuntimeEvent, 4096),
    ...(item.summary !== undefined ? { summary: parseWorkflowRuntimeSummary(item.summary) } : {}),
    ...(item.preferenceUpdates !== undefined
      ? { preferenceUpdates: list(item.preferenceUpdates, parseWorkflowPreferenceUpdate, 256) }
      : {}),
    ...(item.pausedConversationIds !== undefined
      ? { pausedConversationIds: list(item.pausedConversationIds, id, 128) }
      : {}),
    ...(item.inputRuns !== undefined
      ? {
          inputRuns: list(
            item.inputRuns,
            (value) => {
              const row = record(value, ['inputId', 'status'])
              return { inputId: id(row.inputId), status: id(row.status) }
            },
            4096
          )
        }
      : {})
  }
  // A coalesced notification can carry different revisions for one member's model and
  // permission. Never raise one field's revision just to combine it with the other field.
  const preferenceNodes = new Map<string, { conversationId: string; fields: Set<string> }>()
  for (const update of result.preferenceUpdates ?? []) {
    let node = preferenceNodes.get(update.nodeId)
    if (!node) {
      node = { conversationId: update.conversationId, fields: new Set() }
      preferenceNodes.set(update.nodeId, node)
    }
    if (node.conversationId !== update.conversationId || preferenceNodes.size > 128)
      throw new Error('Invalid organization preference update identity')
    for (const field of ['modelId', 'permissionMode'] as const) {
      if (!Object.hasOwn(update, field)) continue
      if (node.fields.has(field)) throw new Error('Duplicate organization preference update')
      node.fields.add(field)
    }
  }
  const inputIds = new Set(result.inputs.map((input) => input.id))
  if (
    (result.summary !== undefined &&
      (result.inputs.length > 0 ||
        (result.inputRuns?.length ?? 0) > 0 ||
        result.summary.structureRevision > result.sequence ||
        result.summary.conversationChanges.some((change) => change.sequence > result.sequence))) ||
    result.inputs.some((input) => input.instanceId !== result.instanceId) ||
    result.inputRuns?.some((run) => !inputIds.has(run.inputId)) ||
    result.events.some(
      (event) => event.instanceId !== result.instanceId || event.sequence > result.sequence
    ) ||
    inputIds.size !== result.inputs.length ||
    result.events.some(
      (event, index) => index > 0 && event.sequence <= result.events[index - 1].sequence
    )
  )
    throw new Error('Invalid organization runtime snapshot identity or cursor')
  return result
}

function parseWorkflowRuntimeSummary(value: unknown): WorkflowRuntimeSummary {
  const item = record(value, ['pendingByNode', 'conversationChanges', 'structureRevision'])
  const pendingByNode = list(
    item.pendingByNode,
    (value) => {
      const row = record(value, ['nodeId', 'count'])
      return { nodeId: id(row.nodeId), count: integer(row.count) }
    },
    128
  )
  // This recovery index includes former members and grows with historical conversation count.
  // An item cap would reject the entire notification and silently lose terminal facts. Transport
  // remains byte-accounted; consumers retain cursors only for their local conversations.
  const conversationChanges = list(
    item.conversationChanges,
    (value) => {
      const row = record(value, ['conversationId', 'sequence'])
      return { conversationId: id(row.conversationId), sequence: integer(row.sequence) }
    },
    Number.POSITIVE_INFINITY
  )
  if (
    new Set(pendingByNode.map((row) => row.nodeId)).size !== pendingByNode.length ||
    new Set(conversationChanges.map((row) => row.conversationId)).size !==
      conversationChanges.length
  )
    throw new Error('Duplicate organization runtime summary identity')
  return { pendingByNode, conversationChanges, structureRevision: integer(item.structureRevision) }
}

function parseWorkflowPreferenceUpdate(value: unknown): WorkflowPreferenceUpdate {
  const item = record(
    value,
    ['nodeId', 'conversationId', 'organizationRevision'],
    ['modelId', 'permissionMode']
  )
  const revision = integer(item.organizationRevision)
  if (
    revision === 0 ||
    (item.modelId === undefined && item.permissionMode === undefined) ||
    (item.permissionMode !== undefined &&
      item.permissionMode !== 'default' &&
      item.permissionMode !== 'custom' &&
      item.permissionMode !== 'full')
  )
    throw new Error('Invalid organization preference update')
  return {
    nodeId: id(item.nodeId),
    conversationId: id(item.conversationId),
    organizationRevision: revision,
    ...(item.modelId !== undefined ? { modelId: id(item.modelId) } : {}),
    ...(item.permissionMode !== undefined ? { permissionMode: item.permissionMode } : {})
  }
}
export function parseWorkflowMessageSource(value: unknown): WorkflowMessageSource {
  const item = record(value, ['inputId', 'instanceId', 'workflowName', 'sources'])
  const sources = list(
    item.sources,
    (value) => {
      const source = record(
        value,
        ['nodeId', 'nodeName', 'conversationId', 'conversationTitle'],
        ['content']
      )
      return {
        nodeId: id(source.nodeId),
        nodeName: text(source.nodeName, 512),
        conversationId: id(source.conversationId),
        conversationTitle: text(source.conversationTitle, 4096),
        ...('content' in source ? { content: text(source.content) } : {})
      }
    },
    512
  )
  if (!sources.length) throw new Error('Organization message must identify its source')
  return {
    inputId: id(item.inputId),
    instanceId: id(item.instanceId),
    workflowName: text(item.workflowName, 512),
    sources
  }
}
