import { parseWorkflowDefinition, type WorkflowNode } from '@mycopilot/protocol'
import { createWorkflow } from './workflowAuthoring'

export const WORKFLOW_NODE_CLIPBOARD_TYPE = 'application/x-captain-workflow-node'

export function serializeWorkflowNode(node: WorkflowNode): string {
  return JSON.stringify({ type: WORKFLOW_NODE_CLIPBOARD_TYPE, version: 1, node })
}

export function readWorkflowNodeClipboard(value: string): WorkflowNode | null {
  if (!value || value.length > 2_000_000) return null
  try {
    const data = JSON.parse(value)
    if (data?.type !== WORKFLOW_NODE_CLIPBOARD_TYPE || data.version !== 1) return null
    const [node] = parseWorkflowDefinition({ ...createWorkflow(), nodes: [data.node] }).nodes
    return node
  } catch {
    return null
  }
}

export function duplicateWorkflowNode(
  source: WorkflowNode,
  position: { x: number; y: number }
): WorkflowNode {
  const node = { ...structuredClone(source), x: position.x, y: position.y, id: crypto.randomUUID() }
  return node
}
