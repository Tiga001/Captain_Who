import {
  workflowMemberNameKey,
  type WorkflowDefinition,
  type WorkflowAgentNode
} from '@mycopilot/protocol'
export const NODE_WIDTH = 212
export const NODE_HEIGHT = 52
export const WORKFLOW_DRAG_TYPE = 'application/x-captain-workflow-template'
export function createWorkflow(): WorkflowDefinition {
  return {
    schemaVersion: 1,
    id: crypto.randomUUID(),
    name: '',
    description: '',
    background: '',
    nodes: [],
    departments: [],
    viewport: { x: 0, y: 0, zoom: 1 }
  }
}
export function workflowNodeSize(_node?: { kind: unknown }) {
  void _node
  return { width: NODE_WIDTH, height: NODE_HEIGHT, portY: NODE_HEIGHT / 2 }
}
/** Give newly added or pasted members a usable name without changing existing members. */
export function availableWorkflowMemberName(
  name: string,
  nodes: readonly { name: string }[]
): string {
  const names = new Set(nodes.map((node) => workflowMemberNameKey(node.name)))
  if (!workflowMemberNameKey(name) || !names.has(workflowMemberNameKey(name))) return name
  let suffix = 2
  let candidate: string
  do {
    const ending = ` (${suffix++})`
    candidate = `${Array.from(name)
      .slice(0, 128 - ending.length)
      .join('')}${ending}`
  } while (names.has(workflowMemberNameKey(candidate)))
  return candidate
}

export function createWorkflowNode(
  name: string,
  x: number,
  y: number,
  modelConfigId: string | null = null,
  existingNodes: readonly { name: string }[] = []
): WorkflowAgentNode {
  return {
    id: crypto.randomUUID(),
    kind: 'agent',
    name: availableWorkflowMemberName(name.slice(0, 128), existingNodes),
    permissionMode: 'default',
    modelConfigId,
    receives: '',
    task: '',
    delivers: '',
    rank: 1,
    managementRole: 'member',
    departmentId: null,
    x,
    y
  }
}
