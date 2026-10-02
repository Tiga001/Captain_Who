import type { WorkflowDefinition, WorkflowInstanceBinding } from '@mycopilot/protocol'
export interface WorkflowNeighborNode {
  nodeId: string
  name: string
  conversationId: string
}
/** All other bound members can communicate; membership carries no routing restrictions. */
export function workflowNeighborNodes(
  definition: WorkflowDefinition,
  bindings: readonly WorkflowInstanceBinding[],
  nodeId: string
): WorkflowNeighborNode[] {
  const conversations = new Map(bindings.map((binding) => [binding.nodeId, binding.conversationId]))
  return definition.nodes.flatMap((node) => {
    const conversationId = conversations.get(node.id)
    return node.kind === 'agent' && node.id !== nodeId && conversationId
      ? [{ nodeId: node.id, name: node.name, conversationId }]
      : []
  })
}
