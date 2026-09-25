import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import fixture from '../../../../../packages/protocol/fixtures/workflow-definition-v1.json'
import {
  createWorkflowNode,
  nodeFlows,
  removeWorkflowFlows
} from '../../features/workflows/workflowAuthoring'

describe('workflow graph editing', () => {
  it('allows blank nodes independently of a model or saved template', () => {
    const node = createWorkflowNode('Research', 150, 80)
    expect(node.templateId).toBeNull()
    expect(node.name).toBe('Research')
    expect(node.task).toBe('')
  })
  it('removes deleted flow membership without silently relaxing quantity limits', () => {
    const graph = parseWorkflowDefinition(fixture)
    graph.nodes[0].inputRule = {
      mode: 'custom',
      min: 0,
      max: 0,
      required: ['entry'],
      groups: [{ id: 'group', flowIds: ['revise'], min: 1, max: 1 }]
    }
    const edited = removeWorkflowFlows(graph, new Set(['entry']))
    expect(edited.nodes[0].inputRule.required).toEqual([])
    expect(edited.nodes[0].inputRule.groups[0].min).toBe(1)
    expect(nodeFlows(edited, 'implement', 'input').map((flow) => flow.id)).toEqual(['revise'])
    expect(graph.flows).toHaveLength(4)
    expect(graph.nodes[0].inputRule.required).toEqual(['entry'])
  })
})
