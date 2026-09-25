/** Workflow definitions only. No execution capability is granted by saving a graph. */
export interface WorkflowGroup {
  id: string
  flowIds: string[]
  min: number
  max: number
}
export interface WorkflowRule {
  mode: 'all' | 'one' | 'any' | 'exact' | 'range' | 'custom'
  min: number
  max: number
  required: string[]
  groups: WorkflowGroup[]
}
interface WorkflowNodeBase {
  id: string
  name: string
  x: number
  y: number
}
export interface WorkflowAgentNode extends WorkflowNodeBase {
  kind: 'agent'
  templateId: string | null
  modelConfigId: string | null
  receives: string
  task: string
  delivers: string
}
export interface WorkflowUserNode extends WorkflowNodeBase {
  kind: 'user'
  task: string
}
/** Assemble input before deciding how to deliver it to a busy agent. */
export interface WorkflowInputGateNode extends WorkflowNodeBase {
  kind: 'inputGate'
  /** Individual consumes one message; batch consumes one message from every incoming flow. */
  processingMode: 'individual' | 'batch'
  /** Applies to complete inputs only, including complete batches. */
  busyPolicy: 'queue' | 'inject'
}
export interface WorkflowOutputGateNode extends WorkflowNodeBase {
  kind: 'outputGate'
  selection: WorkflowRule
}
export type WorkflowNode =
  WorkflowAgentNode | WorkflowUserNode | WorkflowInputGateNode | WorkflowOutputGateNode
export type WorkflowGateNode = WorkflowInputGateNode | WorkflowOutputGateNode
export type WorkflowEndpoint = { kind: 'boundary' } | { kind: 'node'; nodeId: string }
export interface WorkflowAnchor {
  side: 'left' | 'right' | 'top' | 'bottom'
  /** Relative position along the side, from left/top to right/bottom. */
  offset: number
}
export interface WorkflowFlow {
  id: string
  name: string
  source: WorkflowEndpoint
  target: WorkflowEndpoint
  sourceAnchor?: WorkflowAnchor
  targetAnchor?: WorkflowAnchor
}
export interface WorkflowBoundaryPositions {
  input: { x: number; y: number }
  output: { x: number; y: number }
}
/** Boundary cards are layout only. They never become executable worker nodes. */
export function getDefaultWorkflowBoundaryPositions(
  nodes: readonly { x: number; y: number }[]
): WorkflowBoundaryPositions {
  if (!nodes.length) return { input: { x: 80, y: 220 }, output: { x: 760, y: 220 } }
  const clamp = (value: number) => Math.max(-100000, Math.min(100000, value))
  const y = clamp(
    (Math.min(...nodes.map((node) => node.y)) + Math.max(...nodes.map((node) => node.y))) / 2
  )
  return {
    input: { x: clamp(Math.min(...nodes.map((node) => node.x)) - 280), y },
    output: { x: clamp(Math.max(...nodes.map((node) => node.x)) + 464), y }
  }
}
export interface WorkflowDefinition {
  schemaVersion: 1
  id: string
  name: string
  description: string
  background: string
  nodes: WorkflowNode[]
  flows: WorkflowFlow[]
  nextFlowSequence?: number
  viewport: { x: number; y: number; zoom: number }
  boundaryPositions: WorkflowBoundaryPositions
}
export interface WorkflowIssue {
  code: string
  subject: string
}
export interface WorkflowRecord {
  definition: WorkflowDefinition
  /** Effective availability after validation; this does not describe a running workflow. */
  enabled: boolean
  revision: number
  updatedAt: number
  issues: WorkflowIssue[]
}
export type WorkflowRequest =
  | { operation: 'list' }
  | { operation: 'validate'; definition: WorkflowDefinition }
  | { operation: 'save'; definition: WorkflowDefinition; expectedRevision: number }
  | { operation: 'delete'; id: string; expectedRevision: number }
  | { operation: 'setEnabled'; id: string; enabled: boolean; expectedRevision: number }
export interface WorkflowResponse {
  records: WorkflowRecord[]
  issues: WorkflowIssue[]
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
function endpoint(value: unknown): WorkflowEndpoint {
  const kind = (value as { kind?: unknown } | null)?.kind
  const item = object(value, kind === 'boundary' ? ['kind'] : ['kind', 'nodeId'])
  if (kind === 'boundary') return { kind }
  if (kind === 'node') return { kind, nodeId: text(item.nodeId) }
  throw new Error('Invalid workflow endpoint')
}
function anchor(value: unknown): WorkflowAnchor {
  const item = object(value, ['side', 'offset'])
  if (!['left', 'right', 'top', 'bottom'].includes(String(item.side)))
    throw new Error('Invalid workflow anchor side')
  const offset = number(item.offset)
  if (offset < 0 || offset > 1) throw new Error('Invalid workflow anchor offset')
  return { side: item.side as WorkflowAnchor['side'], offset }
}
function boundaryPositions(value: unknown): WorkflowBoundaryPositions {
  const item = object(value, ['input', 'output'])
  const point = (value: unknown) => {
    const position = object(value, ['x', 'y'])
    const x = number(position.x),
      y = number(position.y)
    if (Math.abs(x) > 100000 || Math.abs(y) > 100000)
      throw new Error('Invalid workflow boundary coordinate')
    return { x, y }
  }
  return { input: point(item.input), output: point(item.output) }
}
function rule(value: unknown): WorkflowRule {
  const item = object(value, ['mode', 'min', 'max', 'required', 'groups'])
  if (
    typeof item.mode !== 'string' ||
    !['all', 'one', 'any', 'exact', 'range', 'custom'].includes(item.mode)
  )
    throw new Error('Invalid workflow rule mode')
  return {
    mode: item.mode as WorkflowRule['mode'],
    min: integer(item.min),
    max: integer(item.max),
    required: array(item.required, text),
    groups: array(item.groups, (value) => {
      const group = object(value, ['id', 'flowIds', 'min', 'max'])
      return {
        id: text(group.id),
        flowIds: array(group.flowIds, text),
        min: integer(group.min),
        max: integer(group.max)
      }
    })
  }
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
      'flows',
      'viewport',
      'boundaryPositions',
      'nextFlowSequence'
    ],
    ['nextFlowSequence']
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
          ? ['templateId', 'modelConfigId', 'receives', 'task', 'delivers']
          : kind === 'inputGate'
            ? ['processingMode', 'busyPolicy']
            : kind === 'user'
              ? ['task']
              : ['selection']
      const node = object(value, [...common, ...fields], kind === 'user' ? ['task'] : [])
      const base = {
        id: text(node.id),
        name: text(node.name),
        x: number(node.x),
        y: number(node.y)
      }
      if (kind === 'agent') {
        const templateId = node.templateId === null ? null : text(node.templateId)
        const modelConfigId = node.modelConfigId === null ? null : text(node.modelConfigId)
        if (templateId !== null && modelConfigId !== null)
          throw new Error('A template workflow node cannot override its model')
        return {
          ...base,
          kind,
          templateId,
          modelConfigId,
          receives: text(node.receives),
          task: text(node.task),
          delivers: text(node.delivers)
        }
      }
      if (kind === 'user')
        return { ...base, kind, task: node.task === undefined ? '' : text(node.task) }
      if (kind === 'inputGate') {
        if (node.processingMode !== 'individual' && node.processingMode !== 'batch')
          throw new Error('Invalid input processing mode')
        if (node.busyPolicy !== 'queue' && node.busyPolicy !== 'inject')
          throw new Error('Invalid busy agent policy')
        return { ...base, kind, processingMode: node.processingMode, busyPolicy: node.busyPolicy }
      }
      if (kind === 'outputGate') return { ...base, kind, selection: rule(node.selection) }
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
    ...(item.nextFlowSequence !== undefined
      ? { nextFlowSequence: flowSequence(item.nextFlowSequence) }
      : {}),
    nodes,
    boundaryPositions: boundaryPositions(item.boundaryPositions),
    flows: array(item.flows, (value) => {
      const flow = object(
        value,
        ['id', 'name', 'source', 'target', 'sourceAnchor', 'targetAnchor'],
        ['sourceAnchor', 'targetAnchor']
      )
      return {
        id: text(flow.id),
        name: text(flow.name),
        source: endpoint(flow.source),
        target: endpoint(flow.target),
        ...(flow.sourceAnchor !== undefined ? { sourceAnchor: anchor(flow.sourceAnchor) } : {}),
        ...(flow.targetAnchor !== undefined ? { targetAnchor: anchor(flow.targetAnchor) } : {})
      }
    }),
    viewport: { x: number(viewport.x), y: number(viewport.y), zoom: number(viewport.zoom) }
  }
  if (new TextEncoder().encode(JSON.stringify(result)).length > 2_000_000)
    throw new Error('Workflow exceeds 2 MB')
  return result
}

function flowSequence(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1)
    throw new Error('Invalid workflow flow sequence')
  return value
}
export function parseWorkflowRequest(value: unknown): WorkflowRequest {
  const op = (value as { operation?: unknown } | null)?.operation
  if (op === 'list') {
    object(value, ['operation'])
    return { operation: op }
  }
  if (op === 'validate' || op === 'save') {
    const item = object(
      value,
      op === 'save' ? ['operation', 'definition', 'expectedRevision'] : ['operation', 'definition']
    )
    const definition = parseWorkflowDefinition(item.definition)
    return op === 'save'
      ? { operation: op, definition, expectedRevision: integer(item.expectedRevision) }
      : { operation: op, definition }
  }
  if (op === 'delete') {
    const item = object(value, ['operation', 'id', 'expectedRevision'])
    return { operation: op, id: text(item.id), expectedRevision: integer(item.expectedRevision, 1) }
  }
  if (op === 'setEnabled') {
    const item = object(value, ['operation', 'id', 'enabled', 'expectedRevision'])
    return {
      operation: op,
      id: text(item.id),
      enabled: boolean(item.enabled),
      expectedRevision: integer(item.expectedRevision, 1)
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
  const data = object(value, ['records', 'issues'])
  const records = array(
    data.records,
    (value) => {
      const item = object(
        value,
        ['definition', 'enabled', 'revision', 'updatedAt', 'issues'],
        ['enabled']
      )
      // Early v1 records predate the availability switch; omission is safe, while
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
  return { records, issues: issues(data.issues) }
}
