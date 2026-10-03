/** Organization definitions only. Saving member configuration grants no execution capability. */
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
  /** Higher values represent more senior ranks; omitted means rank 1. */
  rank?: number
  managementRole?: WorkflowManagementRole
  /** Direct department; ancestors are inherited through the department tree. */
  departmentId?: string | null
}
export type WorkflowManagementRole = 'member' | 'organization_admin' | 'department_admin'
export interface WorkflowDepartment {
  id: string
  name: string
  parentId: string | null
  x: number
  y: number
  width: number
  height: number
}
export interface WorkflowAgentNode extends WorkflowNodeBase {
  kind: 'agent'
  permissionMode: 'default' | 'custom' | 'full'
  modelConfigId: string | null
  receives: string
  task: string
  delivers: string
}
export type WorkflowNode = WorkflowAgentNode
export interface WorkflowDefinition {
  schemaVersion: 1
  id: string
  name: string
  description: string
  background: string
  nodes: WorkflowNode[]
  departments?: WorkflowDepartment[]
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
  | { operation: 'list' }
  | { operation: 'validate'; definition: WorkflowDefinition }
  | {
      operation: 'save'
      definition: WorkflowDefinition
      expectedRevision: number
      expectedDraftRevision?: number
    }
  | { operation: 'delete'; id: string; expectedRevision: number }
  | { operation: 'setInstanceEnabled'; id: string; enabled: boolean; expectedRevision: number }
  | { operation: 'listInstances' }
  | {
      operation: 'saveInstance'
      id: string
      /** Creation provenance only. Existing organizations edit their own definition. */
      templateId?: string
      definition?: WorkflowDefinition
      name: string
      color: string
      projectId?: string | null
      bindings: { nodeId: string; conversationId: string | null }[]
      expectedRevision: number
      expectedTemplateRevision?: number
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
/** Most recent continuous activity interval, derived from durable turn timestamps. */
export interface WorkflowActivity {
  startedAt: number
  /** Null while any turn in this interval is still active. */
  completedAt: number | null
}
export interface WorkflowInstance {
  id: string
  /** Independent, editable organization configuration. */
  definition: WorkflowDefinition
  /** Creation provenance; never a runtime dependency. */
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
  /** Absent/null until an organization has a recorded activity interval. */
  activity?: WorkflowActivity | null
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
    throw new Error('Invalid organization object')
  const item = value as Record<string, unknown>
  if (
    Object.keys(item).some((key) => !keys.includes(key)) ||
    keys.some((key) => !optionalKeys.includes(key) && !(key in item))
  )
    throw new Error('Invalid organization fields')
  return item
}
function text(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid organization text')
  return value
}
function boolean(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid organization boolean')
  return value
}
function integer(value: unknown, minimum = 0): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum)
    throw new Error('Invalid organization integer')
  return value
}
function number(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value))
    throw new Error('Invalid organization coordinate')
  return value
}
function array<T>(value: unknown, parse: (item: unknown) => T, max = 512): T[] {
  if (!Array.isArray(value) || value.length > max)
    throw new Error('Invalid organization collection')
  return value.map(parse)
}
/** Parse one member independently, for editor clipboard operations. */
export function parseWorkflowNode(value: unknown): WorkflowNode {
  const kind = (value as { kind?: unknown } | null)?.kind
  const common = ['kind', 'id', 'name', 'x', 'y', 'rank', 'managementRole', 'departmentId']
  const fields =
    kind === 'agent'
      ? ['permissionMode', 'modelConfigId', 'receives', 'task', 'delivers']
      : ['task']
  const node = object(value, [...common, ...fields], ['rank', 'managementRole', 'departmentId'])
  const rank = node.rank === undefined ? undefined : integer(node.rank, 1)
  if (rank !== undefined && rank > 99) throw new Error('Invalid organization rank')
  const managementRole = node.managementRole
  if (
    managementRole !== undefined &&
    managementRole !== 'member' &&
    managementRole !== 'organization_admin' &&
    managementRole !== 'department_admin'
  )
    throw new Error('Invalid organization management role')
  const base: WorkflowNodeBase = {
    id: text(node.id),
    name: text(node.name),
    x: number(node.x),
    y: number(node.y),
    ...(rank !== undefined ? { rank } : {}),
    ...(managementRole !== undefined ? { managementRole } : {}),
    ...(node.departmentId !== undefined
      ? { departmentId: node.departmentId === null ? null : text(node.departmentId) }
      : {})
  }
  if (kind === 'agent') {
    const modelConfigId = node.modelConfigId === null ? null : text(node.modelConfigId)
    const permissionMode = node.permissionMode
    if (permissionMode !== 'default' && permissionMode !== 'custom' && permissionMode !== 'full')
      throw new Error('Invalid organization permission mode')
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
  throw new Error('Invalid organization node kind')
}

export function parseWorkflowDefinition(value: unknown): WorkflowDefinition {
  const item = object(
    value,
    [
      'schemaVersion',
      'id',
      'name',
      'description',
      'background',
      'nodes',
      'departments',
      'viewport'
    ],
    ['departments']
  )
  if (item.schemaVersion !== 1) throw new Error('Unsupported organization version')
  const viewport = object(item.viewport, ['x', 'y', 'zoom'])
  const nodes = array(item.nodes, parseWorkflowNode, 128)
  const result: WorkflowDefinition = {
    schemaVersion: 1,
    id: text(item.id),
    name: text(item.name),
    description: text(item.description),
    background: text(item.background),
    nodes,
    ...(item.departments !== undefined
      ? {
          departments: array(
            item.departments,
            (value) => {
              const department = object(value, [
                'id',
                'name',
                'parentId',
                'x',
                'y',
                'width',
                'height'
              ])
              return {
                id: text(department.id),
                name: text(department.name),
                parentId: department.parentId === null ? null : text(department.parentId),
                x: number(department.x),
                y: number(department.y),
                width: number(department.width),
                height: number(department.height)
              }
            },
            64
          )
        }
      : {}),
    viewport: { x: number(viewport.x), y: number(viewport.y), zoom: number(viewport.zoom) }
  }
  validateWorkflowDepartments(result)
  if (new TextEncoder().encode(JSON.stringify(result)).length > 2_000_000)
    throw new Error('Organization exceeds 2 MB')
  return result
}

/** Match Rust str::trim (Unicode White_Space), then per-character Unicode lowercase; display text is preserved. */
export function workflowMemberNameKey(name: string): string {
  return Array.from(name.replace(/^\p{White_Space}+|\p{White_Space}+$/gu, ''))
    .map((character) => character.toLowerCase())
    .join('')
}

/** Enforce names on writes, while keeping old definitions readable for correction. */
export function validateWorkflowMemberNames(definition: WorkflowDefinition): void {
  const names = new Set<string>()
  for (const node of definition.nodes) {
    const key = workflowMemberNameKey(node.name)
    if (!key) continue // Empty names retain the existing incomplete-draft behavior.
    if (names.has(key)) throw new Error('organization_duplicate_member_name')
    names.add(key)
  }
}

/** Department paths use '/' separators and unique names under each parent. */
export function validateWorkflowDepartmentNames(definition: WorkflowDefinition): void {
  const siblings = new Map<string | null, Set<string>>()
  for (const department of definition.departments ?? []) {
    if (department.name.includes('/')) throw new Error('organization_department_name_separator')
    const key = workflowMemberNameKey(department.name)
    if (!key) continue
    const names = siblings.get(department.parentId) ?? new Set<string>()
    if (names.has(key)) throw new Error('organization_duplicate_department_name')
    names.add(key)
    siblings.set(department.parentId, names)
  }
}

function validateWorkflowDepartments(definition: WorkflowDefinition): void {
  const departments = definition.departments ?? []
  const byId = new Map(departments.map((department) => [department.id, department]))
  const nodeIds = new Set(definition.nodes.map((node) => node.id))
  if (byId.size !== departments.length) throw new Error('Duplicate organization department')
  for (const department of departments) {
    if (
      !department.id ||
      department.id.trim() !== department.id ||
      department.id.length > 256 ||
      [...department.id].some((character) => {
        const code = character.charCodeAt(0)
        return code <= 31 || (code >= 127 && code <= 159)
      }) ||
      nodeIds.has(department.id)
    )
      throw new Error('Invalid organization department identifier')
    if (
      new TextEncoder().encode(department.name).length > 512 ||
      Math.abs(department.x) > 100_000 ||
      Math.abs(department.y) > 100_000 ||
      department.width < 80 ||
      department.height < 64 ||
      department.width > 100_000 ||
      department.height > 100_000
    )
      throw new Error('Invalid organization department bounds or name')
    const visited = new Set([department.id])
    let parentId = department.parentId
    while (parentId !== null) {
      const parent = byId.get(parentId)
      if (!parent || visited.has(parentId)) throw new Error('Invalid organization department tree')
      visited.add(parentId)
      parentId = parent.parentId
    }
  }
  for (const node of definition.nodes) {
    if (
      node.departmentId !== undefined &&
      node.departmentId !== null &&
      !byId.has(node.departmentId)
    )
      throw new Error('Unknown organization department')
  }
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
  if (op === 'list' || op === 'listInstances') {
    object(value, ['operation'])
    return { operation: op }
  }
  if (op === 'validate' || op === 'save') {
    const item = object(
      value,
      op === 'save'
        ? ['operation', 'definition', 'expectedRevision', 'expectedDraftRevision']
        : ['operation', 'definition'],
      ['expectedDraftRevision']
    )
    const definition = parseWorkflowDefinition(item.definition)
    if (op === 'save') {
      validateWorkflowMemberNames(definition)
      validateWorkflowDepartmentNames(definition)
    }
    return op === 'save'
      ? {
          operation: op,
          definition,
          expectedRevision: integer(item.expectedRevision),
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
        'definition',
        'name',
        'color',
        'projectId',
        'bindings',
        'expectedRevision',
        'expectedTemplateRevision'
      ],
      ['projectId', 'templateId', 'expectedTemplateRevision', 'definition']
    )
    const expectedRevision = integer(item.expectedRevision)
    if (expectedRevision > 0 && item.definition === undefined)
      throw new Error('Organization updates require an independent definition')
    if (expectedRevision === 0 && item.definition === undefined && item.templateId === undefined)
      throw new Error('Organization creation requires a definition or template')
    if ((item.templateId === undefined) !== (item.expectedTemplateRevision === undefined))
      throw new Error('Organization template provenance requires its revision')
    const definition =
      item.definition === undefined ? undefined : parseWorkflowDefinition(item.definition)
    if (definition) {
      validateWorkflowMemberNames(definition)
      validateWorkflowDepartmentNames(definition)
    }
    return {
      operation: op,
      id: text(item.id),
      ...(item.templateId !== undefined ? { templateId: text(item.templateId) } : {}),
      ...(definition ? { definition } : {}),
      name: text(item.name),
      color: text(item.color),
      ...(item.projectId !== undefined
        ? { projectId: item.projectId === null ? null : text(item.projectId) }
        : {}),
      expectedRevision,
      ...(item.expectedTemplateRevision !== undefined
        ? { expectedTemplateRevision: integer(item.expectedTemplateRevision, 1) }
        : {}),
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
    const definition = parseWorkflowDefinition(item.definition)
    validateWorkflowMemberNames(definition)
    validateWorkflowDepartmentNames(definition)
    return {
      operation: op,
      definition,
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
  throw new Error('Invalid organization operation')
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
      'drafts',
      'invalidRecords',
      'invalidDrafts',
      'affectedConversationIds',
      'runtime',
      'nodeMessages'
    ],
    [
      'instances',
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
        throw new Error('An enabled organization cannot have validation issues')
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
    throw new Error('Duplicate organization records')
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
    if (new Set(ids).size !== ids.length) throw new Error('Duplicate organization records')
  }
  if (
    invalidDrafts &&
    new Set(invalidDrafts.map((draft) => draft.id)).size !== invalidDrafts.length
  )
    throw new Error('Duplicate invalid organization drafts')
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
    throw new Error('Invalid organization recovery reason')
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
      'definition',
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
      'running',
      'activity'
    ],
    ['projectId', 'activity']
  )
  return {
    id: text(item.id),
    definition: parseWorkflowDefinition(item.definition),
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
    ...(item.activity !== undefined
      ? { activity: item.activity === null ? null : parseWorkflowActivity(item.activity) }
      : {}),
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

function parseWorkflowActivity(value: unknown): WorkflowActivity {
  const item = object(value, ['startedAt', 'completedAt'])
  const startedAt = integer(item.startedAt)
  const completedAt = item.completedAt === null ? null : integer(item.completedAt)
  if (completedAt !== null && completedAt < startedAt)
    throw new Error('Invalid organization activity interval')
  return { startedAt, completedAt }
}
