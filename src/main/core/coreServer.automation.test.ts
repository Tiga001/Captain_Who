import { beforeEach, describe, expect, it, vi } from 'vitest'

const rpcStart = vi.hoisted(() => vi.fn())
const rpcStop = vi.hoisted(() => vi.fn())
const rpcBeginShutdown = vi.hoisted(() => vi.fn())
const rpcIsRunning = vi.hoisted(() => vi.fn())
const rpcRequest = vi.hoisted(() => vi.fn())
const rpcRequestDuringShutdown = vi.hoisted(() => vi.fn())
const onNotification = vi.hoisted(() => vi.fn())

vi.mock('./jsonRpcClient', () => ({
  CoreJsonRpcClient: class {
    readonly start = rpcStart
    readonly stop = rpcStop
    readonly beginShutdown = rpcBeginShutdown
    readonly isRunning = rpcIsRunning
    readonly request = rpcRequest
    readonly requestDuringShutdown = rpcRequestDuringShutdown
    readonly onNotification = onNotification
  }
}))

import { CoreServer } from './coreServer'

describe('CoreServer Automation notifications', () => {
  beforeEach(() => {
    rpcStart.mockReset()
    rpcStop.mockReset()
    rpcBeginShutdown.mockReset()
    rpcIsRunning.mockReset().mockReturnValue(false)
    rpcRequest.mockReset()
    rpcRequestDuringShutdown.mockReset()
    onNotification.mockReset().mockReturnValue(() => undefined)
  })

  it('replays a validated startup resync to a later Main IPC subscriber exactly once', () => {
    const receivers = new Map<string, (params: unknown) => void>()
    onNotification.mockImplementation((method, handler) => {
      receivers.set(method, handler)
      return () => undefined
    })
    const server = new CoreServer()
    server.start()
    receivers.get('automation.resync')?.({
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
    expect(onNotification).toHaveBeenCalledTimes(2)

    unsubscribe()
    receivers.get('automation.resync')?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 8,
      occurredAt: 101
    })
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it('does not cache or deliver malformed startup resync payloads', () => {
    const receivers = new Map<string, (params: unknown) => void>()
    onNotification.mockImplementation((method, handler) => {
      receivers.set(method, handler)
      return () => undefined
    })
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const server = new CoreServer()
    server.start()
    receivers.get('automation.resync')?.({
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

  it('closes lazy Core request admission even when shutdown finds no live child', async () => {
    const server = new CoreServer()

    await server.shutdown()

    expect(rpcBeginShutdown).toHaveBeenCalledOnce()
    expect(rpcIsRunning).toHaveBeenCalledOnce()
    expect(rpcRequest).not.toHaveBeenCalled()
  })

  it('uses the no-restart request path to gracefully stop an already-running Core', async () => {
    rpcIsRunning.mockReturnValue(true)
    rpcRequestDuringShutdown.mockResolvedValue({ stopped: true, timedOut: false })
    const server = new CoreServer()

    await server.shutdown()

    expect(rpcBeginShutdown).toHaveBeenCalledOnce()
    expect(rpcRequestDuringShutdown).toHaveBeenCalledOnce()
    expect(rpcRequest).not.toHaveBeenCalled()
    expect(rpcStop).toHaveBeenCalledOnce()
  })
})
