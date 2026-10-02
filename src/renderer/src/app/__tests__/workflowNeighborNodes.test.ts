import { describe, expect, it } from 'vitest'
import {
  createWorkflow,
  createWorkflowNode,
  createWorkflowUser
} from '../../features/workflows/workflowAuthoring'
import { workflowNeighborNodes } from '../../features/workflows/workflowNeighborNodes'

describe('workflow collaboration members', () => {
  it('lists every other bound agent without requiring a connection', () => {
    const graph = createWorkflow()
    const a = createWorkflowNode('A', 0, 0),
      b = createWorkflowNode('B', 200, 0),
      c = createWorkflowNode('C', 400, 0)
    graph.nodes = [a, b, c, createWorkflowUser('Human', 600, 0)]
    const bindings = [
      { nodeId: a.id, conversationId: 'chat-a' },
      { nodeId: b.id, conversationId: 'chat-b' },
      { nodeId: c.id, conversationId: 'chat-c' }
    ]
    expect(workflowNeighborNodes(graph, bindings, a.id)).toEqual([
      { nodeId: b.id, name: 'B', conversationId: 'chat-b' },
      { nodeId: c.id, name: 'C', conversationId: 'chat-c' }
    ])
    expect(workflowNeighborNodes(graph, bindings.slice(0, 1), a.id)).toEqual([])
  })
})
