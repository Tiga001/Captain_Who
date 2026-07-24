import type { AgentSteerRunInput, AgentSteerRunOutput } from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer agent steering client', () => {
  beforeEach(() => rpcRequest.mockReset())

  it('routes the complete text guidance identity through agent.steerRun', async () => {
    const input = {
      conversationId: 'conversation-1',
      expectedRunId: 'run-1',
      clientMessageId: 'client-guidance-1',
      content: 'Use the updated constraint.'
    } satisfies AgentSteerRunInput
    const output = {
      guidanceId: 'guidance-1',
      status: 'queued'
    } satisfies AgentSteerRunOutput
    rpcRequest.mockResolvedValue(output)

    await expect(new CoreServer().steerRun(input)).resolves.toEqual(output)
    expect(rpcRequest).toHaveBeenCalledWith('agent.steerRun', input)
  })
})
