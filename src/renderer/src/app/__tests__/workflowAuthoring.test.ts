import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import {
  createWorkflow,
  createWorkflowNode,
  createWorkflowUser
} from '../../features/workflows/workflowAuthoring'

describe('independent workflow members', () => {
  it('creates valid empty drafts and independent agents without routing settings', () => {
    const definition = createWorkflow()
    const agent = createWorkflowNode('Writer', 20, 40, 'model')
    const user = createWorkflowUser('Human', 300, 40)
    definition.nodes.push(agent, user)
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    expect(agent).toMatchObject({
      permissionMode: 'default',
      modelConfigId: 'model',
      receives: '',
      task: '',
      delivers: ''
    })
    expect(user).not.toHaveProperty('modelConfigId')
    expect(definition).not.toHaveProperty('flows')
    expect(definition).not.toHaveProperty('boundaryPositions')
    expect(agent.id).not.toBe(user.id)
  })
})
