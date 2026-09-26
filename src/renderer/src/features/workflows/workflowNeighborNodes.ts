import type { WorkflowDefinition, WorkflowInstanceBinding } from '@mycopilot/protocol'

export interface WorkflowNeighborNode {
  nodeId: string
  name: string
  conversationId: string
}

/**
 * Bound agents that send to (upstream) or receive from (downstream) one node. Logic gates are
 * traversed; user nodes and the user entry have no conversation and end the walk.
 */
export function workflowNeighborNodes(
  definition: WorkflowDefinition,
  bindings: readonly WorkflowInstanceBinding[],
  nodeId: string
): { upstream: WorkflowNeighborNode[]; downstream: WorkflowNeighborNode[] } {
  const nodesById = new Map(definition.nodes.map((node) => [node.id, node]))
  const conversationByNode = new Map(
    bindings.map((binding) => [binding.nodeId, binding.conversationId])
  )

  const walk = (direction: 'upstream' | 'downstream') => {
    const found = new Set<string>()
    const visited = new Set([nodeId])
    const pending = [nodeId]
    while (pending.length) {
      const current = pending.pop()!
      for (const flow of definition.flows) {
        const [from, to] =
          direction === 'downstream' ? [flow.source, flow.target] : [flow.target, flow.source]
        if (from.kind !== 'node' || from.nodeId !== current || to.kind !== 'node') continue
        if (visited.has(to.nodeId)) continue
        visited.add(to.nodeId)
        const node = nodesById.get(to.nodeId)
        if (node?.kind === 'agent') found.add(node.id)
        if (node?.kind === 'inputGate' || node?.kind === 'outputGate') pending.push(node.id)
      }
    }
    return definition.nodes.flatMap((node) => {
      const conversationId = conversationByNode.get(node.id)
      if (!found.has(node.id) || !conversationId) return []
      return [{ nodeId: node.id, name: node.name.trim() || node.id, conversationId }]
    })
  }
  return { upstream: walk('upstream'), downstream: walk('downstream') }
}
