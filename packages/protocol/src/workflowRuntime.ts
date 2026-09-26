/** Host-owned workflow delivery facts; notification cursors never grant execution authority. */
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
  flowId: string
  pathFlowIds: string[]
  content: string
  createdAt: number
}
export type WorkflowInputStatus =
  | 'pending'
  | 'claimed'
  | 'applied'
  | 'waiting_user'
  | 'completed'
  | 'paused'
  | 'failed'
  | 'invalidated'
export interface WorkflowRuntimeInput {
  id: string
  instanceId: string
  nodeId: string
  conversationId: string | null
  executionVersion: string
  content: string
  messages: WorkflowSourceMessage[]
  busyPolicy: 'queue' | 'inject'
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
  flowIds: string[]
  kind: string
  createdAt: number
}
export interface WorkflowRuntimeSnapshot {
  instanceId: string
  sequence: number
  inputs: WorkflowRuntimeInput[]
  events: WorkflowRuntimeEvent[]
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
    throw new Error('Invalid workflow runtime object')
  const item = value as Record<string, unknown>
  if (
    Object.keys(item).some((key) => !keys.includes(key) && !optionalKeys.includes(key)) ||
    keys.some((key) => !(key in item))
  )
    throw new Error('Invalid workflow runtime fields')
  return item
}
function text(value: unknown, max = 2_000_000): string {
  if (typeof value !== 'string' || value.length > max)
    throw new Error('Invalid workflow runtime text')
  return value
}
function id(value: unknown): string {
  const result = text(value, 512)
  if (
    !result.trim() ||
    [...result].some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127)
  )
    throw new Error('Invalid workflow runtime ID')
  return result
}
function integer(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0)
    throw new Error('Invalid workflow runtime sequence or time')
  return value
}
function list<T>(value: unknown, parse: (item: unknown) => T, max = 1024): T[] {
  if (!Array.isArray(value) || value.length > max)
    throw new Error('Invalid workflow runtime collection')
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
    'flowId',
    'pathFlowIds',
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
    flowId: id(item.flowId),
    pathFlowIds: list(item.pathFlowIds, id, 512),
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
    'busyPolicy',
    'status',
    'runId',
    'deliveryId',
    'createdAt',
    'error'
  ])
  if (item.busyPolicy !== 'queue' && item.busyPolicy !== 'inject')
    throw new Error('Invalid workflow input policy')
  const statuses: string[] = [
    'pending',
    'claimed',
    'applied',
    'waiting_user',
    'completed',
    'paused',
    'failed',
    'invalidated'
  ]
  if (typeof item.status !== 'string' || !statuses.includes(item.status))
    throw new Error('Invalid workflow input status')
  const input: WorkflowRuntimeInput = {
    id: id(item.id),
    instanceId: id(item.instanceId),
    nodeId: id(item.nodeId),
    conversationId: nullableId(item.conversationId),
    executionVersion: id(item.executionVersion),
    content: text(item.content),
    messages: list(item.messages, parseWorkflowSourceMessage, 512),
    busyPolicy: item.busyPolicy,
    status: item.status as WorkflowInputStatus,
    runId: nullableId(item.runId),
    deliveryId: nullableId(item.deliveryId),
    createdAt: integer(item.createdAt),
    error: item.error === null ? null : text(item.error)
  }
  if (
    !input.messages.length ||
    input.messages.some(
      (message) => message.instanceId !== input.instanceId || message.targetNodeId !== input.nodeId
    )
  )
    throw new Error('Workflow input source identity mismatch')
  return input
}
export function parseWorkflowRuntimeEvent(value: unknown): WorkflowRuntimeEvent {
  const item = record(value, ['sequence', 'instanceId', 'inputId', 'flowIds', 'kind', 'createdAt'])
  return {
    sequence: integer(item.sequence),
    instanceId: id(item.instanceId),
    inputId: nullableId(item.inputId),
    flowIds: list(item.flowIds, id, 512),
    kind: id(item.kind),
    createdAt: integer(item.createdAt)
  }
}
export function parseWorkflowRuntimeSnapshot(value: unknown): WorkflowRuntimeSnapshot {
  const item = record(value, ['instanceId', 'sequence', 'inputs', 'events'])
  const result = {
    instanceId: id(item.instanceId),
    sequence: integer(item.sequence),
    inputs: list(item.inputs, parseWorkflowRuntimeInput, 4096),
    events: list(item.events, parseWorkflowRuntimeEvent, 4096)
  }
  if (
    result.inputs.some((input) => input.instanceId !== result.instanceId) ||
    result.events.some(
      (event) => event.instanceId !== result.instanceId || event.sequence > result.sequence
    ) ||
    new Set(result.inputs.map((input) => input.id)).size !== result.inputs.length ||
    result.events.some(
      (event, index) => index > 0 && event.sequence <= result.events[index - 1].sequence
    )
  )
    throw new Error('Invalid workflow runtime snapshot identity or cursor')
  return result
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
  if (!sources.length) throw new Error('Workflow message must identify its source')
  return {
    inputId: id(item.inputId),
    instanceId: id(item.instanceId),
    workflowName: text(item.workflowName, 512),
    sources
  }
}
