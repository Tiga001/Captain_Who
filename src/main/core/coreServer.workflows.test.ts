import { beforeEach, describe, expect, it, vi } from 'vitest'
import fixture from '../../../packages/protocol/fixtures/workflow-definition-v1.json'
import { parseWorkflowDefinition, type WorkflowRequest } from '@mycopilot/protocol'
const rpcRequest = vi.hoisted(() => vi.fn())
vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    request = rpcRequest
    onNotification = vi.fn()
  }
}))
import { CoreServer } from './coreServer'

describe('workflow IPC contract boundary', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
  })
  it('forwards a versioned definition and the optimistic revision to Rust', async () => {
    const definition = parseWorkflowDefinition(fixture)
    if (definition.nodes[0].kind !== 'agent') throw new Error('Expected agent')
    definition.nodes[0].modelConfigId = 'implementation-model'
    definition.boundaryPositions = { input: { x: 100, y: 140 } }
    const response = {
      records: [{ definition, enabled: false, revision: 2, updatedAt: 42, issues: [] }],
      issues: []
    }
    rpcRequest.mockResolvedValue(response)
    const server = new CoreServer()
    const request = { operation: 'save' as const, definition, expectedRevision: 1 }
    await expect(server.requestWorkflows(request)).resolves.toEqual(response)
    expect(rpcRequest).toHaveBeenCalledExactlyOnceWith('agent.workflows.request', request)
  })
  it('forwards the instance switch with its revision', async () => {
    const server = new CoreServer()
    for (const enabled of [true, false]) {
      const request = {
        operation: 'setInstanceEnabled' as const,
        id: fixture.id,
        enabled,
        expectedRevision: 2
      }
      const response = {
        records: [{ definition: fixture, enabled: true, revision: 1, updatedAt: 42, issues: [] }],
        issues: [],
        instances: [
          {
            id: fixture.id,
            templateId: fixture.id,
            templateRevision: 1,
            name: 'Review',
            color: '#4A82E8',
            bindings: [],
            revision: 3,
            updatedAt: 42,
            needsReview: false,
            enabled,
            running: false
          }
        ]
      }
      rpcRequest.mockResolvedValue(response)
      await expect(server.requestWorkflows(request)).resolves.toEqual(response)
      expect(rpcRequest).toHaveBeenLastCalledWith('agent.workflows.request', request)
    }
  })
  it('preserves isolated recovery entries through the Host boundary for both workflow pages', async () => {
    const invalid = {
      id: 'legacy',
      name: '1234',
      revision: 2,
      updatedAt: 42,
      reason: 'incompatible_definition'
    }
    const response = {
      records: [],
      issues: [],
      instances: [],
      invalidRecords: [invalid],
      invalidDrafts: [{ ...invalid, baseRevision: 2 }]
    }
    rpcRequest.mockResolvedValue(response)
    const server = new CoreServer()
    for (const operation of ['list', 'listInstances'] as const) {
      await expect(server.requestWorkflows({ operation })).resolves.toEqual(response)
    }
  })
  it('enforces switch types before RPC and rejects invalid enabled records after RPC', async () => {
    const server = new CoreServer()
    const request = {
      operation: 'setInstanceEnabled' as const,
      id: fixture.id,
      enabled: true,
      expectedRevision: 1
    }
    for (const invalid of [
      { ...request, expectedRevision: 0 },
      { ...request, enabled: 'true' },
      { ...request, extra: true }
    ]) {
      expect(() => server.requestWorkflows(invalid as unknown as WorkflowRequest)).toThrow()
    }
    expect(rpcRequest).not.toHaveBeenCalled()
    for (const enabled of [null, 'false']) {
      rpcRequest.mockResolvedValue({
        records: [{ definition: fixture, enabled, revision: 1, updatedAt: 42, issues: [] }],
        issues: []
      })
      await expect(server.requestWorkflows({ operation: 'list' })).rejects.toThrow()
    }
    rpcRequest.mockResolvedValue({
      records: [
        {
          definition: fixture,
          enabled: true,
          revision: 1,
          updatedAt: 42,
          issues: [{ code: 'node_task', subject: 'review' }]
        }
      ],
      issues: []
    })
    await expect(server.requestWorkflows({ operation: 'list' })).rejects.toThrow(
      'cannot have validation issues'
    )
    const conflict = Object.assign(new Error('Revision conflict'), { code: -32009 })
    rpcRequest.mockRejectedValue(conflict)
    await expect(server.requestWorkflows(request)).rejects.toBe(conflict)
  })
  it('rejects invalid request before RPC and malformed responses after RPC', async () => {
    const server = new CoreServer()
    expect(() =>
      server.requestWorkflows({ operation: 'delete', id: 'x', expectedRevision: -1 })
    ).toThrow()
    const contradictory = parseWorkflowDefinition(fixture)
    if (contradictory.nodes[0].kind !== 'agent') throw new Error('Expected agent')
    Object.assign(contradictory.nodes[0], { permissionMode: 'unknown' })
    contradictory.nodes[0].modelConfigId = 'model-override'
    expect(() =>
      server.requestWorkflows({
        operation: 'save',
        definition: contradictory,
        expectedRevision: 0
      })
    ).toThrow('Invalid workflow permission mode')
    expect(rpcRequest).not.toHaveBeenCalled()
    const invalidLayout = parseWorkflowDefinition(fixture)
    invalidLayout.boundaryPositions.input.x = 100001
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
