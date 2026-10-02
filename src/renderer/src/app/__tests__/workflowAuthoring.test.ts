import { describe, expect, it } from 'vitest'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
import { createWorkflow, createWorkflowNode } from '../../features/workflows/workflowAuthoring'

describe('independent organization members', () => {
  it('creates non-conflicting default names regardless of case and outer spaces', () => {
    const members = [{ name: '　Untitled Node ' }, { name: 'untitled node (2)' }]
    expect(createWorkflowNode('Untitled node', 0, 0, null, members).name).toBe('Untitled node (3)')
  })

  it('creates valid empty drafts and independent agents without routing settings', () => {
    const definition = createWorkflow()
    const agent = createWorkflowNode('Writer', 20, 40, 'model')
    const user = createWorkflowNode('Human', 300, 40)
    definition.nodes.push(agent, user)
    expect(parseWorkflowDefinition(definition)).toEqual(definition)
    expect(agent).toMatchObject({
      permissionMode: 'default',
      modelConfigId: 'model',
      receives: '',
      task: '',
      delivers: ''
    })
    expect(user).toHaveProperty('modelConfigId', null)
    expect(definition).not.toHaveProperty('flows')
    expect(definition).not.toHaveProperty('boundaryPositions')
    expect(agent.id).not.toBe(user.id)
  })
})
