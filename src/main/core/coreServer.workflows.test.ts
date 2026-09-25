import { beforeEach, describe, expect, it, vi } from 'vitest'
import fixture from '../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { parseWorkflowDefinition } from '@mycopilot/protocol'
const rpcRequest = vi.hoisted(() => vi.fn())
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    request = rpcRequest
    onNotification = vi.fn()
  }
}))
import { CoreServer } from './coreServer'

describe('workflow IPC contract boundary', () => {
  beforeEach(() => rpcRequest.mockReset())
  it('forwards a versioned definition and the optimistic revision to Rust', async () => {
    const definition = parseWorkflowDefinition(fixture)
    definition.nodes[0].modelConfigId = 'implementation-model'
    definition.boundaryPositions = { input: { x: 100, y: 140 }, output: { x: 1100, y: 420 } }
    const response = {
      records: [{ definition, revision: 2, updatedAt: 42, issues: [] }],
      issues: []
    }
    rpcRequest.mockResolvedValue(response)
    const server = new CoreServer()
    const request = { operation: 'save' as const, definition, expectedRevision: 1 }
    await expect(server.requestWorkflows(request)).resolves.toEqual(response)
    expect(rpcRequest).toHaveBeenCalledExactlyOnceWith('agent.workflows.request', request)
  })
  it('normalizes an older stored v1 model field after RPC without inventing an execution model', async () => {
    const legacy = structuredClone(fixture)
    Reflect.deleteProperty(legacy.nodes[0], 'modelConfigId')
    Reflect.deleteProperty(legacy, 'boundaryPositions')
    rpcRequest.mockResolvedValue({
      records: [{ definition: legacy, revision: 1, updatedAt: 42, issues: [] }],
      issues: []
    })
    const output = await new CoreServer().requestWorkflows({ operation: 'list' })
    expect(output.records[0].definition.nodes[0].modelConfigId).toBeNull()
    expect(output.records[0].definition.boundaryPositions).toEqual(fixture.boundaryPositions)
  })
  it('rejects invalid request before RPC and malformed responses after RPC', async () => {
    const server = new CoreServer()
    expect(() =>
      server.requestWorkflows({ operation: 'delete', id: 'x', expectedRevision: -1 })
    ).toThrow()
    const contradictory = parseWorkflowDefinition(fixture)
    contradictory.nodes[0].templateId = 'template-review'
    contradictory.nodes[0].modelConfigId = 'model-override'
    expect(() =>
      server.requestWorkflows({
        operation: 'save',
        definition: contradictory,
        expectedRevision: 0
      })
    ).toThrow('cannot override')
    expect(rpcRequest).not.toHaveBeenCalled()
    const invalidLayout = parseWorkflowDefinition(fixture)
    invalidLayout.boundaryPositions.output.x = 100001
    expect(() =>
      server.requestWorkflows({ operation: 'save', definition: invalidLayout, expectedRevision: 0 })
    ).toThrow()
    expect(rpcRequest).not.toHaveBeenCalled()
    rpcRequest.mockResolvedValue({
      records: [
        {
          definition: { ...fixture, boundaryPositions: null },
          revision: 1,
          updatedAt: 42,
          issues: []
        }
      ],
      issues: []
    })
    await expect(server.requestWorkflows({ operation: 'list' })).rejects.toThrow()
    rpcRequest.mockResolvedValue({ records: [{}], issues: [] })
    await expect(server.requestWorkflows({ operation: 'list' })).rejects.toThrow()
  })
})
