/** Workflow definitions only. No execution capability is granted by saving a graph. */
export interface WorkflowGroup {
  id: string
  flowIds: string[]
  min: number
  max: number
}
export interface WorkflowRule {
  mode: 'all' | 'one' | 'any' | 'range' | 'custom'
  min: number
  max: number
  required: string[]
  groups: WorkflowGroup[]
}
export interface WorkflowNode {
  id: string
  name: string
  templateId: string | null
  /** Explicit model for a standalone node. Template nodes use their template's model. */
  modelConfigId: string | null
  receives: string
  task: string
  delivers: string
  inputRule: WorkflowRule
  outputRule: WorkflowRule
  x: number
  y: number
}
export type WorkflowEndpoint = { kind: 'boundary' } | { kind: 'node'; nodeId: string }
export interface WorkflowFlow {
  id: string
  name: string
  source: WorkflowEndpoint
  target: WorkflowEndpoint
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
  viewport: { x: number; y: number; zoom: number }
  boundaryPositions: WorkflowBoundaryPositions
}
export interface WorkflowIssue {
  code: string
  subject: string
}
export interface WorkflowRecord {
  definition: WorkflowDefinition
  revision: number
  updatedAt: number
  issues: WorkflowIssue[]
}
export type WorkflowRequest =
  | { operation: 'list' }
  | { operation: 'validate'; definition: WorkflowDefinition }
  | { operation: 'save'; definition: WorkflowDefinition; expectedRevision: number }
  | { operation: 'delete'; id: string; expectedRevision: number }
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
    !['all', 'one', 'any', 'range', 'custom'].includes(item.mode)
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
      'boundaryPositions'
    ],
    ['boundaryPositions']
  )
  if (item.schemaVersion !== 1) throw new Error('Unsupported workflow version')
  const viewport = object(item.viewport, ['x', 'y', 'zoom'])
  const nodes = array(
    item.nodes,
    (value) => {
      const node = object(
        value,
        [
          'id',
          'name',
          'templateId',
          'modelConfigId',
          'receives',
          'task',
          'delivers',
          'inputRule',
          'outputRule',
          'x',
          'y'
        ],
        ['modelConfigId']
      )
      const templateId = node.templateId === null ? null : text(node.templateId)
      // Early v1 definitions predate model selection. Normalize their absent
      // field without accepting malformed explicit values or other schema drift.
      const modelConfigId =
        !Object.hasOwn(node, 'modelConfigId') || node.modelConfigId === null
          ? null
          : text(node.modelConfigId)
      if (templateId !== null && modelConfigId !== null)
        throw new Error('A template workflow node cannot override its model')
      return {
        id: text(node.id),
        name: text(node.name),
        templateId,
        modelConfigId,
        receives: text(node.receives),
        task: text(node.task),
        delivers: text(node.delivers),
        inputRule: rule(node.inputRule),
        outputRule: rule(node.outputRule),
        x: number(node.x),
        y: number(node.y)
      }
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
    boundaryPositions: Object.hasOwn(item, 'boundaryPositions')
      ? boundaryPositions(item.boundaryPositions)
      : getDefaultWorkflowBoundaryPositions(nodes),
    flows: array(item.flows, (value) => {
      const flow = object(value, ['id', 'name', 'source', 'target'])
      return {
        id: text(flow.id),
        name: text(flow.name),
        source: endpoint(flow.source),
        target: endpoint(flow.target)
      }
    }),
    viewport: { x: number(viewport.x), y: number(viewport.y), zoom: number(viewport.zoom) }
  }
  if (new TextEncoder().encode(JSON.stringify(result)).length > 2_000_000)
    throw new Error('Workflow exceeds 2 MB')
  return result
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
      const item = object(value, ['definition', 'revision', 'updatedAt', 'issues'])
      return {
        definition: parseWorkflowDefinition(item.definition),
        revision: integer(item.revision, 1),
        updatedAt: integer(item.updatedAt),
        issues: issues(item.issues)
      }
    },
    1000
  )
  if (new Set(records.map((record) => record.definition.id)).size !== records.length)
    throw new Error('Duplicate workflow records')
  return { records, issues: issues(data.issues) }
}
