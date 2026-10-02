import type { WorkflowDefinition, WorkflowAgentNode, WorkflowUserNode } from '@mycopilot/protocol'
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
    viewport: { x: 0, y: 0, zoom: 1 }
  }
}
export function workflowNodeSize(_node?: { kind: unknown }) {
  void _node
  return { width: NODE_WIDTH, height: NODE_HEIGHT, portY: NODE_HEIGHT / 2 }
}
export function createWorkflowNode(
  name: string,
  x: number,
  y: number,
  modelConfigId: string | null = null
): WorkflowAgentNode {
  return {
    id: crypto.randomUUID(),
    kind: 'agent',
    name: name.slice(0, 128),
    permissionMode: 'default',
    modelConfigId,
    receives: '',
    task: '',
    delivers: '',
    x,
    y
  }
}

export function createWorkflowUser(name: string, x: number, y: number): WorkflowUserNode {
  return { id: crypto.randomUUID(), kind: 'user', name: name.slice(0, 128), task: '', x, y }
}
