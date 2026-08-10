import type {
  AgentProviderTransitionOperation,
  AgentProviderTransitionPreflightOutput
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcRequest = vi.hoisted(() => vi.fn())
const onNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly request = rpcRequest
    readonly onNotification = onNotification
  }
}))

import { CoreServer } from './coreServer'

const preflight = {
  conversationId: 'conversation-1',
  targetModelId: 'generic-model',
  decision: 'requires_compaction',
  reason: 'api_provider_changed',
  operationId: 'provider-transition-1',
  transitionToken: 'opaque-token'
} satisfies AgentProviderTransitionPreflightOutput

const running = {
  schemaVersion: 1,
  operationId: 'provider-transition-1',
  conversationId: 'conversation-1',
  targetModelId: 'generic-model',
  coveredThroughMessageId: 'assistant-1',
  status: 'running',
  startedAt: 10
} satisfies AgentProviderTransitionOperation

describe('CoreServer Provider transition client', () => {
  beforeEach(() => {
    rpcRequest.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('routes and validates preflight, start and status identities', async () => {
    rpcRequest
      .mockResolvedValueOnce(preflight)
      .mockResolvedValueOnce(running)
      .mockResolvedValueOnce({ operations: [running] })
    const server = new CoreServer()

    await expect(
      server.preflightProviderTransition({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model'
      })
    ).resolves.toEqual(preflight)
    await expect(
      server.startProviderTransition({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        transitionToken: 'opaque-token'
      })
    ).resolves.toEqual(running)
    await expect(
      server.getProviderTransitionStatus({ conversationId: 'conversation-1' })
    ).resolves.toEqual({ operations: [running] })

    expect(rpcRequest).toHaveBeenNthCalledWith(1, 'agent.preflightProviderTransition', {
      conversationId: 'conversation-1',
      targetModelId: 'generic-model'
    })
    expect(rpcRequest).toHaveBeenNthCalledWith(2, 'agent.startProviderTransition', {
      conversationId: 'conversation-1',
      targetModelId: 'generic-model',
      transitionToken: 'opaque-token'
    })
    expect(rpcRequest).toHaveBeenNthCalledWith(3, 'agent.getProviderTransitionStatus', {
      conversationId: 'conversation-1'
    })
  })

  it('rejects mismatched and private response fields with a stable Host-safe error', async () => {
    rpcRequest.mockResolvedValue({
      ...preflight,
      conversationId: 'other-conversation',
      continuation: 'private-provider-state'
    })

    await expect(
      new CoreServer().preflightProviderTransition({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model'
      })
    ).rejects.toThrow('Unable to check this model switch. Please try again.')
  })

  it('does not forward arbitrary Core RPC diagnostics', async () => {
    rpcRequest.mockRejectedValue(
      Object.assign(new Error('provider body with sensitive data'), {
        code: -32000,
        data: { continuation: 'private-provider-state' }
      })
    )

    await expect(
      new CoreServer().startProviderTransition({
        conversationId: 'conversation-1',
        targetModelId: 'generic-model',
        transitionToken: 'stale-token'
      })
    ).rejects.toThrow('The model-switch check expired. Please try again.')
  })

  it('strictly parses notifications and ignores invalid payloads', () => {
    const handler = vi.fn()
    new CoreServer().onProviderTransition(handler)
    const notificationHandler = onNotification.mock.calls[0]?.[1]

    notificationHandler(running)
    notificationHandler({ ...running, reasoningContent: 'private-reasoning' })

    expect(onNotification).toHaveBeenCalledWith('agent.providerTransition', expect.any(Function))
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith(running)
  })
})
