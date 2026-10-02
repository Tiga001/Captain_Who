/** Workflow definitions only. No execution capability is granted by saving a graph. */
import {
  parseWorkflowRuntimeSnapshot,
  parseWorkflowNodeMessages,
  type WorkflowRuntimeSnapshot,
  type WorkflowNodeMessages
} from './workflowRuntime'
interface WorkflowNodeBase {
  id: string
  name: string
  x: number
  y: number
}
export interface WorkflowAgentNode extends WorkflowNodeBase {
  kind: 'agent'
  permissionMode: 'default' | 'custom' | 'full'
  modelConfigId: string | null
  receives: string
  task: string
  delivers: string
}
export interface WorkflowUserNode extends WorkflowNodeBase {
  kind: 'user'
  task: string
}
export type WorkflowNode = WorkflowAgentNode | WorkflowUserNode
export interface WorkflowDefinition {
  schemaVersion: 1
  id: string
  name: string
  description: string
  background: string
  nodes: WorkflowNode[]
  viewport: { x: number; y: number; zoom: number }
}
export interface WorkflowIssue {
  code: string
  subject: string
}
export interface WorkflowRecord {
  definition: WorkflowDefinition
  /** Derived template readiness: true exactly when validation has no issues. */
  enabled: boolean
  revision: number
  updatedAt: number
  issues: WorkflowIssue[]
}
export type WorkflowRequest =
  | { operation: 'runtimeSnapshot'; instanceId: string; afterSequence?: number }
  | { operation: 'nodeMessages'; instanceId: string; nodeId: string; beforeSequence?: number }
  | { operation: 'completeUserInput'; instanceId: string; inputId: string }
  | { operation: 'list' }
  | { operation: 'validate'; definition: WorkflowDefinition }
  | {
      operation: 'save'
      definition: WorkflowDefinition
      expectedRevision: number
      expectedUsageRevision?: string
      expectedDraftRevision?: number
    }
  | { operation: 'delete'; id: string; expectedRevision: number }
  | { operation: 'setInstanceEnabled'; id: string; enabled: boolean; expectedRevision: number }
  | { operation: 'listInstances' }
  | {
      operation: 'saveInstance'
      id: string
      templateId: string
      name: string
      color: string
      projectId?: string | null
      bindings: { nodeId: string; conversationId: string | null }[]
      expectedRevision: number
      expectedTemplateRevision: number
    }
  | { operation: 'deleteInstance'; id: string; expectedRevision: number }
  | {
      operation: 'saveDraft'
      definition: WorkflowDefinition
      expectedRevision: number
      expectedDraftRevision: number
    }
  | { operation: 'deleteDraft'; id: string; expectedDraftRevision: number }
  | { operation: 'duplicate'; id: string; expectedRevision: number; newId: string; name: string }
export interface WorkflowInstanceBinding {
  nodeId: string
  conversationId: string
}
export interface WorkflowInstance {
  id: string
  templateId: string
  templateRevision: number
  name: string
  color: string
  /** Default destination for new conversations only; existing bindings may span projects. */
  projectId?: string | null
  bindings: WorkflowInstanceBinding[]
  revision: number
  updatedAt: number
  needsReview: boolean
  enabled: boolean
  /** Activity in an enabled instance; no client operation starts graph execution. */
  running: boolean
}
export interface WorkflowTemplateUsage {
  templateId: string
  usageRevision: string
  instances: { id: string; name: string; running: boolean; projectNames: string[] }[]
}
export interface WorkflowEditingDraft {
  definition: WorkflowDefinition
  baseRevision: number
  revision: number
  updatedAt: number
}
/** Readable metadata for a preserved definition that the current editor cannot open. */
export interface WorkflowInvalidRecord {
  id: string
  name: string
  revision: number
  updatedAt: number
  reason: 'incompatible_definition' | 'invalid_definition'
}
export interface WorkflowInvalidDraft extends WorkflowInvalidRecord {
  baseRevision: number
}
export interface WorkflowResponse {
  runtime?: WorkflowRuntimeSnapshot
  nodeMessages?: WorkflowNodeMessages
  records: WorkflowRecord[]
  issues: WorkflowIssue[]
  instances?: WorkflowInstance[]
  usages?: WorkflowTemplateUsage[]
  drafts?: WorkflowEditingDraft[]
  invalidRecords?: WorkflowInvalidRecord[]
  invalidDrafts?: WorkflowInvalidDraft[]
  affectedConversationIds?: string[]
}
export const WORKFLOW_REQUEST_METHOD = 'agent.workflows.request'

function object(
  value: unknown,
  keys: string[],
  optionalKeys: string[] = []
): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid workflow object')
  const item = value as Record<string, unknown>
  if (
    Object.keys(item).some((key) => !keys.includes(key)) ||
    keys.some((key) => !optionalKeys.includes(key) && !(key in item))
  )
    throw new Error('Invalid workflow fields')
  return item
}
function text(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid workflow text')
  return value
}
function boolean(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid workflow boolean')
  return value
}
function integer(value: unknown, minimum = 0): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum)
    throw new Error('Invalid workflow integer')
  return value
}
function number(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value))
    throw new Error('Invalid workflow coordinate')
  return value
}
function array<T>(value: unknown, parse: (item: unknown) => T, max = 512): T[] {
  if (!Array.isArray(value) || value.length > max) throw new Error('Invalid workflow collection')
  return value.map(parse)
}
export function parseWorkflowDefinition(value: unknown): WorkflowDefinition {
  const item = object(
    value,
    ['schemaVersion', 'id', 'name', 'description', 'background', 'nodes', 'viewport'],
    []
  )
  if (item.schemaVersion !== 1) throw new Error('Unsupported workflow version')
  const viewport = object(item.viewport, ['x', 'y', 'zoom'])
  const nodes = array(
    item.nodes,
    (value): WorkflowNode => {
      const kind = (value as { kind?: unknown } | null)?.kind
      const common = ['kind', 'id', 'name', 'x', 'y']
      const fields =
        kind === 'agent'
          ? ['permissionMode', 'modelConfigId', 'receives', 'task', 'delivers']
          : ['task']
      const node = object(value, [...common, ...fields], kind === 'user' ? ['task'] : [])
      const base = {
        id: text(node.id),
        name: text(node.name),
        x: number(node.x),
        y: number(node.y)
      }
      if (kind === 'agent') {
        const modelConfigId = node.modelConfigId === null ? null : text(node.modelConfigId)
        const permissionMode = node.permissionMode
        if (
          permissionMode !== 'default' &&
          permissionMode !== 'custom' &&
          permissionMode !== 'full'
        )
          throw new Error('Invalid workflow permission mode')
        return {
          ...base,
          kind,
          permissionMode,
          modelConfigId,
          receives: text(node.receives),
          task: text(node.task),
          delivers: text(node.delivers)
        }
      }
      if (kind === 'user')
        return { ...base, kind, task: node.task === undefined ? '' : text(node.task) }
      throw new Error('Invalid workflow node kind')
    },
    128
  )
  const result: WorkflowDefinition = {
    schemaVersion: 1,
    id: text(item.id),
    name: text(item.name),
    description: text(item.description),
    background: text(item.background),
    nodes,
    viewport: { x: number(viewport.x), y: number(viewport.y), zoom: number(viewport.zoom) }
  }
  if (new TextEncoder().encode(JSON.stringify(result)).length > 2_000_000)
    throw new Error('Workflow exceeds 2 MB')
  return result
}

export function parseWorkflowRequest(value: unknown): WorkflowRequest {
  const op = (value as { operation?: unknown } | null)?.operation
  if (op === 'nodeMessages') {
    const item = object(
      value,
      ['operation', 'instanceId', 'nodeId', 'beforeSequence'],
      ['beforeSequence']
    )
    return {
      operation: op,
      instanceId: text(item.instanceId),
      nodeId: text(item.nodeId),
      ...(item.beforeSequence !== undefined ? { beforeSequence: integer(item.beforeSequence) } : {})
    }
  }
  if (op === 'runtimeSnapshot') {
    const item = object(value, ['operation', 'instanceId', 'afterSequence'], ['afterSequence'])
    return {
      operation: op,
      instanceId: text(item.instanceId),
      ...(item.afterSequence !== undefined ? { afterSequence: integer(item.afterSequence) } : {})
    }
  }
  if (op === 'completeUserInput') {
    const item = object(value, ['operation', 'instanceId', 'inputId'])
    return { operation: op, instanceId: text(item.instanceId), inputId: text(item.inputId) }
  }
  if (op === 'list' || op === 'listInstances') {
    object(value, ['operation'])
    return { operation: op }
  }
  if (op === 'validate' || op === 'save') {
    const item = object(
      value,
      op === 'save'
        ? [
            'operation',
            'definition',
            'expectedRevision',
            'expectedUsageRevision',
            'expectedDraftRevision'
          ]
        : ['operation', 'definition'],
      ['expectedUsageRevision', 'expectedDraftRevision']
    )
    const definition = parseWorkflowDefinition(item.definition)
    return op === 'save'
      ? {
          operation: op,
          definition,
          expectedRevision: integer(item.expectedRevision),
          ...(item.expectedUsageRevision !== undefined
            ? { expectedUsageRevision: text(item.expectedUsageRevision) }
            : {}),
          ...(item.expectedDraftRevision !== undefined
            ? { expectedDraftRevision: integer(item.expectedDraftRevision) }
            : {})
        }
      : { operation: op, definition }
  }
  if (op === 'delete' || op === 'deleteInstance') {
    const item = object(value, ['operation', 'id', 'expectedRevision'])
    return { operation: op, id: text(item.id), expectedRevision: integer(item.expectedRevision, 1) }
  }
  if (op === 'setInstanceEnabled') {
    const item = object(value, ['operation', 'id', 'enabled', 'expectedRevision'])
    return {
      operation: op,
      id: text(item.id),
      enabled: boolean(item.enabled),
      expectedRevision: integer(item.expectedRevision, 1)
    }
  }
  if (op === 'saveInstance') {
    const item = object(
      value,
      [
        'operation',
        'id',
        'templateId',
        'name',
        'color',
        'projectId',
        'bindings',
        'expectedRevision',
        'expectedTemplateRevision'
      ],
      ['projectId']
    )
    return {
      operation: op,
      id: text(item.id),
      templateId: text(item.templateId),
      name: text(item.name),
      color: text(item.color),
      ...(item.projectId !== undefined
        ? { projectId: item.projectId === null ? null : text(item.projectId) }
        : {}),
      expectedRevision: integer(item.expectedRevision),
      expectedTemplateRevision: integer(item.expectedTemplateRevision, 1),
      bindings: array(
        item.bindings,
        (value) => {
          const binding = object(value, ['nodeId', 'conversationId'])
          return {
            nodeId: text(binding.nodeId),
            conversationId: binding.conversationId === null ? null : text(binding.conversationId)
          }
        },
        128
      )
    }
  }
  if (op === 'saveDraft') {
    const item = object(value, [
      'operation',
      'definition',
      'expectedRevision',
      'expectedDraftRevision'
    ])
    return {
      operation: op,
      definition: parseWorkflowDefinition(item.definition),
      expectedRevision: integer(item.expectedRevision, 1),
      expectedDraftRevision: integer(item.expectedDraftRevision)
    }
  }
  if (op === 'deleteDraft') {
    const item = object(value, ['operation', 'id', 'expectedDraftRevision'])
    return {
      operation: op,
      id: text(item.id),
      expectedDraftRevision: integer(item.expectedDraftRevision, 1)
    }
  }
  if (op === 'duplicate') {
    const item = object(value, ['operation', 'id', 'expectedRevision', 'newId', 'name'])
    return {
      operation: op,
      id: text(item.id),
      expectedRevision: integer(item.expectedRevision, 1),
      newId: text(item.newId),
      name: text(item.name)
    }
  }
  throw new Error('Invalid workflow operation')
}
function issues(value: unknown): WorkflowIssue[] {
  return array(
    value,
    (value) => {
      const issue = object(value, ['code', 'subject'])
      return { code: text(issue.code), subject: text(issue.subject) }
    },
    2048
  )
}
export function parseWorkflowResponse(value: unknown): WorkflowResponse {
  const data = object(
    value,
    [
      'records',
      'issues',
      'instances',
      'usages',
      'drafts',
      'invalidRecords',
      'invalidDrafts',
      'affectedConversationIds',
      'runtime',
      'nodeMessages'
    ],
    [
      'instances',
      'usages',
      'drafts',
      'invalidRecords',
      'invalidDrafts',
      'affectedConversationIds',
      'runtime',
      'nodeMessages'
    ]
  )
  const records = array(
    data.records,
    (value) => {
      const item = object(
        value,
        ['definition', 'enabled', 'revision', 'updatedAt', 'issues'],
        ['enabled']
      )
      // Missing readiness is treated as unavailable until the host is updated;
      // explicit malformed values and contradictory effective states are rejected.
      const enabled = Object.hasOwn(item, 'enabled') ? boolean(item.enabled) : false
      const recordIssues = issues(item.issues)
      if (enabled && recordIssues.length)
        throw new Error('An enabled workflow cannot have validation issues')
      return {
        definition: parseWorkflowDefinition(item.definition),
        enabled,
        revision: integer(item.revision, 1),
        updatedAt: integer(item.updatedAt),
        issues: recordIssues
      }
    },
    1000
  )
  if (new Set(records.map((record) => record.definition.id)).size !== records.length)
    throw new Error('Duplicate workflow records')
  const invalidRecords =
    data.invalidRecords === undefined
      ? undefined
      : array(data.invalidRecords, (value) => parseInvalidWorkflow(value, false), 1000)
  const invalidDrafts =
    data.invalidDrafts === undefined
      ? undefined
      : array(data.invalidDrafts, (value) => parseInvalidWorkflow(value, true), 1000)
  if (invalidRecords) {
    const ids = [
      ...records.map((record) => record.definition.id),
      ...invalidRecords.map((record) => record.id)
    ]
    if (new Set(ids).size !== ids.length) throw new Error('Duplicate workflow records')
  }
  if (
    invalidDrafts &&
    new Set(invalidDrafts.map((draft) => draft.id)).size !== invalidDrafts.length
  )
    throw new Error('Duplicate invalid workflow drafts')
  return {
    records,
    ...(data.nodeMessages !== undefined
      ? { nodeMessages: parseWorkflowNodeMessages(data.nodeMessages) }
      : {}),
    ...(data.runtime !== undefined ? { runtime: parseWorkflowRuntimeSnapshot(data.runtime) } : {}),
    issues: issues(data.issues),
    ...(invalidRecords ? { invalidRecords } : {}),
    ...(invalidDrafts ? { invalidDrafts } : {}),
    ...(data.instances !== undefined
      ? { instances: array(data.instances, parseWorkflowInstance, 1000) }
      : {}),
    ...(data.usages !== undefined
      ? {
          usages: array(
            data.usages,
            (value) => {
              const usage = object(value, ['templateId', 'usageRevision', 'instances'])
              return {
                templateId: text(usage.templateId),
                usageRevision: text(usage.usageRevision),
                instances: array(
                  usage.instances,
                  (value) => {
                    const instance = object(value, ['id', 'name', 'running', 'projectNames'])
                    return {
                      id: text(instance.id),
                      name: text(instance.name),
                      running: boolean(instance.running),
                      projectNames: array(instance.projectNames, text, 1000)
                    }
                  },
                  1000
                )
              }
            },
            1000
          )
        }
      : {}),
    ...(data.drafts !== undefined
      ? {
          drafts: array(
            data.drafts,
            (value) => {
              const draft = object(value, ['definition', 'baseRevision', 'revision', 'updatedAt'])
              return {
                definition: parseWorkflowDefinition(draft.definition),
                baseRevision: integer(draft.baseRevision, 1),
                revision: integer(draft.revision, 1),
                updatedAt: integer(draft.updatedAt)
              }
            },
            1000
          )
        }
      : {}),
    ...(data.affectedConversationIds !== undefined
      ? { affectedConversationIds: array(data.affectedConversationIds, text, 1000) }
      : {})
  }
}

function parseInvalidWorkflow(value: unknown, draft: true): WorkflowInvalidDraft
function parseInvalidWorkflow(value: unknown, draft: false): WorkflowInvalidRecord
function parseInvalidWorkflow(
  value: unknown,
  draft: boolean
): WorkflowInvalidRecord | WorkflowInvalidDraft {
  const item = object(value, [
    'id',
    'name',
    'revision',
    'updatedAt',
    'reason',
    ...(draft ? ['baseRevision'] : [])
  ])
  if (item.reason !== 'incompatible_definition' && item.reason !== 'invalid_definition')
    throw new Error('Invalid workflow recovery reason')
  return {
    id: text(item.id),
    name: text(item.name),
    revision: integer(item.revision, 1),
    updatedAt: integer(item.updatedAt),
    reason: item.reason,
    ...(draft ? { baseRevision: integer(item.baseRevision, 1) } : {})
  }
}

function parseWorkflowInstance(value: unknown): WorkflowInstance {
  const item = object(
    value,
    [
      'id',
      'templateId',
      'templateRevision',
      'name',
      'color',
      'projectId',
      'bindings',
      'revision',
      'updatedAt',
      'needsReview',
      'enabled',
      'running'
    ],
    ['projectId']
  )
  return {
    id: text(item.id),
    templateId: text(item.templateId),
    templateRevision: integer(item.templateRevision, 1),
    name: text(item.name),
    color: text(item.color),
    ...(item.projectId !== undefined
      ? { projectId: item.projectId === null ? null : text(item.projectId) }
      : {}),
    revision: integer(item.revision, 1),
    updatedAt: integer(item.updatedAt),
    needsReview: boolean(item.needsReview),
    enabled: boolean(item.enabled),
    running: boolean(item.running),
    bindings: array(
      item.bindings,
      (value) => {
        const binding = object(value, ['nodeId', 'conversationId'])
        return { nodeId: text(binding.nodeId), conversationId: text(binding.conversationId) }
      },
      128
    )
  }
}
