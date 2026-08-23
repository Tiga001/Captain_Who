import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcStart = vi.hoisted(() => vi.fn())
const rpcStop = vi.hoisted(() => vi.fn())
const rpcRequest = vi.hoisted(() => vi.fn())
const onNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly start = rpcStart
    readonly stop = rpcStop
    readonly request = rpcRequest
    readonly onNotification = onNotification
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer Automation notifications', () => {
  beforeEach(() => {
    rpcStart.mockReset()
    rpcStop.mockReset()
    rpcRequest.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('replays a validated startup resync to a later Main IPC subscriber exactly once', () => {
    let receive: ((params: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receive = handler
      return () => undefined
    })
    const server = new CoreServer()
    server.start()
    receive?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 7,
      occurredAt: 100
    })

    const handler = vi.fn()
    const unsubscribe = server.onAutomationResync(handler)
    expect(handler).toHaveBeenCalledTimes(1)
    expect(handler).toHaveBeenCalledWith({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 7,
      occurredAt: 100
    })
    expect(onNotification).toHaveBeenCalledTimes(1)

    unsubscribe()
    receive?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 8,
      occurredAt: 101
    })
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it('does not cache or deliver malformed startup resync payloads', () => {
    let receive: ((params: unknown) => void) | undefined
    onNotification.mockImplementation((_method, handler) => {
      receive = handler
      return () => undefined
    })
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const server = new CoreServer()
    server.start()
    receive?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 7,
      occurredAt: 100,
      privateState: true
    })

    const handler = vi.fn()
    server.onAutomationResync(handler)
    expect(handler).not.toHaveBeenCalled()
    expect(warning).toHaveBeenCalledWith('Ignored invalid Automation resync')
  })

  it('rejects a valid run projection routed from another automation', async () => {
    rpcRequest.mockResolvedValue({
      schemaVersion: 1,
      runId: 'run-1',
      automationId: 'automation-other',
      configRevision: 1,
      triggerKind: 'manual',
      scheduledFor: 10,
      status: 'queued',
      conversationId: null,
      userMessageId: null,
      assistantMessageId: null,
      reportKind: 'unknown',
      resultPreview: null,
      errorCode: null,
      errorMessage: null,
      attention: null,
      createdAt: 10,
      startedAt: null,
      completedAt: null,
      updatedAt: 10
    })

    await expect(
      new CoreServer().runAutomationNow({
        schemaVersion: 1,
        automationId: 'automation-1',
        requestId: 'request-1'
      })
    ).rejects.toThrow('Invalid Automation run response identity')
  })
})
