import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

const request = {
  runId: 'run-owned',
  actionId: 'file-change-action-current',
  approvalScope: 'singleAction' as const
}

function fileChangeExecution(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    actionId: request.actionId,
    actionType: 'file_change',
    toolName: 'apply_patch',
    status: 'applied',
    fileChangeResult: {
      schemaVersion: 1,
      status: 'applied',
      outcome: 'applied',
      transactionId: 'file-change-transaction-current',
      operation: 'update',
      updateStrategy: 'modify',
      filePath: 'README.md',
      additions: 1,
      deletions: 1,
      lineCount: 1,
      byteCount: 4,
      revision: 'content-sha256-v1:target',
      errorCode: null,
      error: null,
      message: null
    },
    agentOutput: {
      content: '',
      status: 'running',
      runId: request.runId,
      events: [],
      toolDefinitions: [],
      proposedActions: []
    },
    ...overrides
  }
}

describe('CoreServer Agent action execution identity', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('accepts a fileChangeResult-only approval response with exact action and Run identity', async () => {
    const output = fileChangeExecution()
    rpcRequest.mockResolvedValue(output)

    await expect(new CoreServer().approveAction(request)).resolves.toEqual(output)
    expect(rpcRequest).toHaveBeenCalledWith('agent.approveAction', request)
  })

  it.each([
    {
      name: 'action',
      output: fileChangeExecution({ actionId: 'file-change-action-forged' })
    },
    {
      name: 'Run',
      output: fileChangeExecution({
        agentOutput: {
          content: '',
          status: 'running',
          runId: 'run-forged',
          events: [],
          toolDefinitions: [],
          proposedActions: []
        }
      })
    }
  ])('rejects a structurally valid response with mismatched $name identity', async ({ output }) => {
    rpcRequest.mockResolvedValue(output)

    await expect(new CoreServer().approveAction(request)).rejects.toThrow(
      'Invalid Agent action execution identity'
    )
  })

  it('applies the same identity check to rejection responses', async () => {
    rpcRequest.mockResolvedValue(fileChangeExecution({ actionId: 'file-change-action-forged' }))

    await expect(
      new CoreServer().rejectAction({ ...request, message: 'Do not apply this change.' })
    ).rejects.toThrow('Invalid Agent action execution identity')
    expect(rpcRequest).toHaveBeenCalledWith('agent.rejectAction', {
      ...request,
      message: 'Do not apply this change.'
    })
  })
})
